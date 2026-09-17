//! The renderer child's loop: one current-thread future that owns every wait.
//!
//! It drains engines, then sleeps until the next timer deadline, a dial
//! completion, or a message from the browser. Commands become engine calls;
//! engine events and replies become wire messages.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use renderer::{DialFailure, Engine, FrameId, MAX_RESPONSE_BODY_BYTES, Stop, TabError};
use tokio::sync::Notify;
use url::Url;

use super::{AssignmentServices, ChannelServices, RendererInput};
use crate::wire::{Command, FromRenderer, RendererAssignmentId, Reply, ResponseStart, ToRenderer};

/// Runs the renderer loop until `Shutdown`, channel close, or stop.
pub(super) async fn run(
    mut inbox: tokio::sync::mpsc::Receiver<RendererInput>,
    outbox: &SyncSender<FromRenderer>,
    services: Arc<ChannelServices>,
    stop: &Arc<Stop>,
    wake: Arc<Notify>,
) {
    let mut engines = HashMap::<RendererAssignmentId, Engine>::new();
    let mut responses = ResponseStreams::default();
    loop {
        if !drain_engines(&mut engines, outbox) || stop.is_set() {
            stop.request();
            break;
        }
        let deadline = engines.values().filter_map(Engine::next_deadline).min();
        tokio::select! {
            received = inbox.recv() => match received {
                Some(RendererInput::Control(ToRenderer::Assign { assignment })) => {
                    if engines.contains_key(&assignment) {
                        stop.request();
                        break;
                    }
                    let assignment_services = Arc::new(AssignmentServices::new(
                        assignment,
                        Arc::clone(&services),
                    ));
                    engines.insert(
                        assignment,
                        Engine::new(assignment_services, Arc::clone(stop), Arc::clone(&wake)),
                    );
                }
                Some(RendererInput::Control(ToRenderer::Release { assignment })) => {
                    responses.release(assignment);
                    if let Some(mut engine) = engines.remove(&assignment) {
                        engine.release();
                    }
                }
                Some(RendererInput::Control(ToRenderer::Request { id, assignment, command })) => {
                    let Some(engine) = engines.get_mut(&assignment) else {
                        stop.request();
                        break;
                    };
                    let (reply, shutdown) = handle_command(engine, command, stop);
                    if !send_to_browser(outbox, FromRenderer::Reply { id, assignment, reply }) {
                        stop.request();
                        break;
                    }
                    if shutdown {
                        break;
                    }
                }
                Some(RendererInput::Control(ToRenderer::ResponseStart { id, response })) => {
                    let Some(engine) = engines.get_mut(&response.assignment) else {
                        stop.request();
                        break;
                    };
                    if responses.start(id, &response, engine).is_err() {
                        stop.request();
                        break;
                    }
                }
                Some(RendererInput::Body { request, bytes }) => {
                    if responses.push(request, &bytes, &mut engines).is_err() {
                        stop.request();
                        break;
                    }
                }
                Some(RendererInput::Control(ToRenderer::ResponseEnd { id })) => {
                    let (assignment, result) = responses.finish(id, &mut engines);
                    let reply = Reply::Unit(result);
                    if !send_to_browser(outbox, FromRenderer::Reply { id, assignment, reply }) {
                        stop.request();
                        break;
                    }
                }
                Some(RendererInput::Control(ToRenderer::ResponseError { id, failure })) => {
                    let (assignment, result) = responses.abort(id, failure, &mut engines);
                    let reply = Reply::Unit(result);
                    if !send_to_browser(outbox, FromRenderer::Reply { id, assignment, reply }) {
                        stop.request();
                        break;
                    }
                }
                // The transport consumes the handshake, response stream, and
                // service replies; none reaches this loop.
                Some(
                    RendererInput::Control(
                        ToRenderer::Hello | ToRenderer::ServiceReply { .. },
                    ),
                ) => {}
                None => break,
            },
            () = wake.notified() => {}
            () = wait_for_deadline(deadline) => {}
        }
    }
    for (_, mut engine) in engines {
        engine.shutdown();
    }
}

/// Runs every engine's ready work and publishes what it produced.
fn drain_engines(
    engines: &mut HashMap<RendererAssignmentId, Engine>,
    outbox: &SyncSender<FromRenderer>,
) -> bool {
    for (assignment, engine) in engines {
        engine.drain_ready();
        if !publish(*assignment, engine, outbox) {
            return false;
        }
    }
    true
}

/// One streamed top-level response: which engine and frame it feeds, and how
/// many bytes it has carried.
struct ActiveResponse {
    assignment: RendererAssignmentId,
    frame: FrameId,
    bytes: usize,
}

/// Streamed responses in flight, keyed by request id.
#[derive(Default)]
struct ResponseStreams(HashMap<u64, ActiveResponse>);

impl ResponseStreams {
    fn start(
        &mut self,
        id: u64,
        response: &ResponseStart,
        engine: &mut Engine,
    ) -> Result<(), TabError> {
        if self.0.contains_key(&id) {
            return Err(stream_error("duplicate response start"));
        }
        let url = Url::parse(&response.final_url).ok();
        engine.open_body(
            response.frame,
            url.as_ref(),
            response.content_type.as_deref(),
            response.content_language.as_deref(),
        )?;
        self.0.insert(
            id,
            ActiveResponse {
                assignment: response.assignment,
                frame: response.frame,
                bytes: 0,
            },
        );
        Ok(())
    }

    fn push(
        &mut self,
        id: u64,
        bytes: &[u8],
        engines: &mut HashMap<RendererAssignmentId, Engine>,
    ) -> Result<(), TabError> {
        let response = self
            .0
            .get_mut(&id)
            .ok_or_else(|| stream_error("body frame without response start"))?;
        response.bytes = response.bytes.saturating_add(bytes.len());
        if response.bytes > MAX_RESPONSE_BODY_BYTES {
            return Err(stream_error("streamed response exceeds body limit"));
        }
        engines
            .get_mut(&response.assignment)
            .ok_or_else(|| stream_error("response assignment is gone"))?
            .write_body(response.frame, bytes)
    }

    fn finish(
        &mut self,
        id: u64,
        engines: &mut HashMap<RendererAssignmentId, Engine>,
    ) -> (RendererAssignmentId, Result<(), TabError>) {
        let response = match self.take(id, "response end without start") {
            Ok(response) => response,
            Err((assignment, error)) => return (assignment, Err(error)),
        };
        let assignment = response.assignment;
        let result =
            engine_mut(engines, assignment).and_then(|engine| engine.end_body(response.frame));
        (assignment, result)
    }

    fn abort(
        &mut self,
        id: u64,
        failure: DialFailure,
        engines: &mut HashMap<RendererAssignmentId, Engine>,
    ) -> (RendererAssignmentId, Result<(), TabError>) {
        let response = match self.take(id, "response error without start") {
            Ok(response) => response,
            Err((assignment, error)) => return (assignment, Err(error)),
        };
        // Stop the frame's parser, or it waits for bytes that will never come
        // and the tab stays loading forever.
        if let Ok(engine) = engine_mut(engines, response.assignment) {
            let _ = engine.abort_body(response.frame);
        }
        (
            response.assignment,
            Err(stream_error(&format!(
                "streamed response failed: {failure:?}"
            ))),
        )
    }

    /// Removes the stream for `id`, defaulting the assignment when the browser
    /// sent no matching start.
    fn take(
        &mut self,
        id: u64,
        missing_start: &str,
    ) -> Result<ActiveResponse, (RendererAssignmentId, TabError)> {
        self.0
            .remove(&id)
            .ok_or_else(|| (RendererAssignmentId::new(0), stream_error(missing_start)))
    }

    fn release(&mut self, assignment: RendererAssignmentId) {
        self.0
            .retain(|_, response| response.assignment != assignment);
    }
}

fn engine_mut(
    engines: &mut HashMap<RendererAssignmentId, Engine>,
    assignment: RendererAssignmentId,
) -> Result<&mut Engine, TabError> {
    engines
        .get_mut(&assignment)
        .ok_or_else(|| stream_error("response assignment is gone"))
}

fn stream_error(message: &str) -> TabError {
    TabError::RendererUnavailable {
        message: message.to_owned(),
    }
}

async fn wait_for_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn handle_command(engine: &mut Engine, command: Command, stop: &Arc<Stop>) -> (Reply, bool) {
    match command {
        Command::Mount { frame, mount } => (Reply::Unit(engine.mount_frame(frame, &mount)), false),
        Command::Eval { frame, source } => (Reply::Text(engine.eval_in(frame, &source)), false),
        Command::ExecuteScript {
            frame,
            source,
            timeout_ms,
        } => {
            let timeout = timeout_ms.map(Duration::from_millis);
            let value = engine.execute_remote_in(frame, &source, timeout);
            (Reply::Value(value), false)
        }
        Command::Shutdown => {
            stop.request();
            (Reply::Unit(Ok(())), true)
        }
    }
}

/// Sends every event one engine produced, returning false when the host is gone.
fn publish(
    assignment: RendererAssignmentId,
    engine: &mut Engine,
    outbox: &SyncSender<FromRenderer>,
) -> bool {
    let Ok(events) = engine.take_events() else {
        return false;
    };
    for (frame, event) in events {
        if !send_to_browser(
            outbox,
            FromRenderer::Event {
                assignment,
                frame,
                event,
            },
        ) {
            return false;
        }
    }
    true
}

fn send_to_browser(outbox: &SyncSender<FromRenderer>, message: FromRenderer) -> bool {
    match outbox.try_send(message) {
        Ok(()) => true,
        Err(
            std::sync::mpsc::TrySendError::Full(_) | std::sync::mpsc::TrySendError::Disconnected(_),
        ) => false,
    }
}
