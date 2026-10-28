use std::sync::mpsc;
use std::time::Duration;

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module, Result};
use url::Url;

use crate::protocol::DialRequest;

pub(super) struct WebModuleResolver;

impl Resolver for WebModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        let base = Url::parse(base).map_err(|_| Error::new_resolving(base, name))?;
        let resolved = Url::parse(name)
            .or_else(|_| base.join(name))
            .map_err(|_| Error::new_resolving(base.as_str(), name))?;
        match resolved.scheme() {
            "http" | "https" => Ok(resolved.into()),
            _ => Err(Error::new_resolving(base.as_str(), name)),
        }
    }
}

pub(super) struct WebModuleLoader;

impl Loader for WebModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        if attributes.is_some() {
            return Err(Error::new_loading(name));
        }
        load_module(ctx, name)
    }
}

fn module_request_blocked(
    world_rc: &std::rc::Rc<std::cell::RefCell<super::World>>,
    ctx: &Ctx<'_>,
    name: &str,
) -> Result<()> {
    let Ok(url) = url::Url::parse(name) else {
        return Ok(());
    };
    let violations = {
        let world = world_rc.borrow();
        world.policy.csp.check_request(
            &world.origin,
            &world.document_url,
            &url,
            crate::csp::Destination::Script,
            None,
        )
    };
    if violations.is_empty() {
        return Ok(());
    }
    let blocked = crate::csp::blocks(&violations);
    if let Ok(target) = super::bindings::main_document(ctx) {
        // The module loader runs inside QuickJS; dispatch later.
        world_rc
            .borrow_mut()
            .queue_csp_violations(target, violations);
    }
    if blocked {
        return Err(Error::new_loading(name));
    }
    Ok(())
}

pub(super) fn load_module<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Module<'js, Declared>> {
    // A JavaScript module graph is fetched before evaluation
    // (<https://html.spec.whatwg.org/multipage/webappapis.html#fetch-a-single-module-script>).
    // QuickJS's host loader is synchronous, so the renderer waits here while
    // the browser process performs the network operation on its I/O runtime.
    let world_rc = super::bindings::world(ctx)?;
    let (services, initiator, referrer_url, referrer_policy, crossorigin, gate) = {
        let world = world_rc.borrow();
        // Every fetch in a module graph keeps the graph root's script fetch
        // options: its `referrerpolicy` and the credentials mode its
        // `crossorigin` state selects. The document records the root element as
        // `current_script` while the graph loads, so the same lookup serves the
        // root and a dynamic `import()` inside a classic script
        // (<https://html.spec.whatwg.org/multipage/webappapis.html#fetch-the-descendants-of-and-link-a-module-script>).
        let options = world
            .state
            .module_fetch_root
            .or(world.state.current_script)
            .map(|element| world.subresource_options(element))
            .unwrap_or_default();
        (
            world.runtime.services.clone(),
            world.origin.ascii_serialization(),
            world.referrer_url.to_string(),
            options
                .referrer_policy
                .or(world.policy.referrer_policy)
                .unwrap_or_default(),
            options.crossorigin,
            crate::RedirectGate::new(
                world.policy.csp.clone(),
                &world.origin,
                crate::csp::Destination::Script,
            ),
        )
    };
    // A module graph fetch is a script request; check it before the renderer
    // waits on the dial
    // (<https://w3c.github.io/webappsec-csp/#should-block-request>).
    module_request_blocked(&world_rc, ctx, name)?;

    let (send, receive) = mpsc::sync_channel(1);
    let mut request = DialRequest::new(
        crate::protocol::RequestPolicy::ModuleScript { crossorigin },
        name,
        &initiator,
        &referrer_url,
    );
    request.referrer = net::Referrer::Client;
    request.csp = Some(Box::new(gate));
    request.referrer_policy = referrer_policy;
    let cancel = services.start_dial(
        request,
        Box::new(move |outcome| {
            let _result = send.send(outcome);
        }),
    );
    let outcome = if let Ok(outcome) = receive.recv_timeout(Duration::from_secs(30)) {
        outcome.map_err(|_| Error::new_loading(name))?
    } else {
        cancel();
        return Err(Error::new_loading(name));
    };
    if outcome.final_url != name
        && let Ok(final_url) = url::Url::parse(&outcome.final_url)
        && let Ok(request_url) = url::Url::parse(name)
    {
        let violations = {
            let world = world_rc.borrow();
            world.policy.csp.check_redirected_request(
                &world.origin,
                &world.document_url,
                &final_url,
                &request_url,
                crate::csp::Destination::Script,
            )
        };
        if !violations.is_empty() {
            let blocked = crate::csp::blocks(&violations);
            if let Ok(target) = super::bindings::main_document(ctx) {
                world_rc
                    .borrow_mut()
                    .queue_csp_violations(target, violations);
            }
            if blocked {
                return Err(Error::new_loading(name));
            }
        }
    }
    if !(200..300).contains(&outcome.status)
        || !super::javascript_module_mime(outcome.content_type.as_deref())
    {
        return Err(Error::new_loading(name));
    }
    let crate::protocol::ResponseBody::Bytes(body) = outcome.body else {
        return Err(Error::new_loading(name));
    };
    Module::declare(ctx.clone(), name, body)
}
