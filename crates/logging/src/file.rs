//! Async batched file sink: a bounded queue plus one writer thread.
//!
//! Records are best-effort. When the queue is full the caller counts a drop
//! instead of blocking a page thread, and the writer emits one warning per
//! batch. Rotation keeps a single `<file>.1` backup. Write failures are
//! ignored: a full disk must not fail the process that is logging.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Records accepted before callers start counting drops.
const QUEUE_CAPACITY: usize = 1024;
/// Longest a record sits in the buffer before the writer flushes.
const BATCH_WINDOW: Duration = Duration::from_millis(250);
/// Records per write when the queue is busy.
const BATCH_RECORDS: usize = 256;
/// Bytes per write when records are large.
const BATCH_BYTES: usize = 64 * 1024;
/// Size that rotates the file to its single backup.
pub(crate) const ROTATE_BYTES: u64 = 8 * 1024 * 1024;
/// Bound on `flush` and shutdown so a stuck disk cannot hang a process.
const TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
    Stop,
}

pub(crate) struct FileSink {
    tx: SyncSender<Message>,
    thread: Option<JoinHandle<()>>,
}

impl FileSink {
    /// Opens `path` (creating parents) and starts the writer thread.
    pub(crate) fn new(path: PathBuf) -> io::Result<Self> {
        let file = open(&path)?;
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let rotation = ROTATE_BYTES;
        let thread = thread::Builder::new()
            .name("logging-file".to_owned())
            .spawn(move || run(&rx, &path, file, rotation))?;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Queues one line; `false` when the queue is full or the writer is gone.
    pub(crate) fn try_send(&self, line: &str) -> bool {
        self.tx.try_send(Message::Line(line.to_owned())).is_ok()
    }

    /// Waits until every accepted record has been written, at most [`TIMEOUT`].
    pub(crate) fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        let mut message = Message::Flush(ack_tx);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.tx.try_send(message) {
                Ok(()) => {
                    let _ = ack_rx.recv_timeout(TIMEOUT);
                    return;
                }
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    message = returned;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }

    /// Queue with no writer thread; used by tests to force a full queue.
    #[cfg(test)]
    pub(crate) fn stalled(capacity: usize) -> (Self, Receiver<Message>) {
        let (tx, rx) = mpsc::sync_channel(capacity);
        (Self { tx, thread: None }, rx)
    }
}

impl Drop for FileSink {
    fn drop(&mut self) {
        if self.tx.try_send(Message::Stop).is_ok()
            && let Some(thread) = self.thread.take()
        {
            let _ = thread.join();
        }
    }
}

/// Buffers lines and writes them in batches until `Stop` or disconnect.
fn run(rx: &Receiver<Message>, path: &Path, mut file: File, rotate_at: u64) {
    let mut buffer = String::with_capacity(BATCH_BYTES);
    let mut records = 0_usize;
    let mut written = file.metadata().map_or(0, |meta| meta.len());
    loop {
        match rx.recv_timeout(BATCH_WINDOW) {
            Ok(Message::Line(line)) => {
                buffer.push_str(line.trim_end_matches(['\r', '\n']));
                buffer.push('\n');
                records += 1;
                if records >= BATCH_RECORDS || buffer.len() >= BATCH_BYTES {
                    written = write_batch(&mut file, path, &buffer, written, rotate_at);
                    buffer.clear();
                    records = 0;
                }
            }
            Ok(Message::Flush(ack)) => {
                written = write_batch(&mut file, path, &buffer, written, rotate_at);
                buffer.clear();
                records = 0;
                let _ = ack.send(());
            }
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => {
                let _ = write_batch(&mut file, path, &buffer, written, rotate_at);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                if records > 0 {
                    written = write_batch(&mut file, path, &buffer, written, rotate_at);
                    buffer.clear();
                    records = 0;
                }
            }
        }
    }
}

/// Writes one batch, rotating first when it would cross the cap.
fn write_batch(file: &mut File, path: &Path, buffer: &str, written: u64, rotate_at: u64) -> u64 {
    if buffer.is_empty() {
        return written;
    }
    let Some(total) = written.checked_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX)) else {
        return written;
    };
    if total > rotate_at {
        return rotate(file, path, buffer);
    }
    if file.write_all(buffer.as_bytes()).is_err() {
        return written;
    }
    total
}

/// Renames the live file to `<file>.1` and starts a fresh one.
fn rotate(file: &mut File, path: &Path, buffer: &str) -> u64 {
    let backup = backup_path(path);
    let _ = fs::remove_file(&backup);
    if fs::rename(path, &backup).is_ok()
        && let Ok(fresh) = File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
    {
        *file = fresh;
    }
    if file.write_all(buffer.as_bytes()).is_err() {
        return 0;
    }
    u64::try_from(buffer.len()).unwrap_or(u64::MAX)
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".1");
    PathBuf::from(backup)
}

fn open(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    File::options().create(true).append(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn temp_path(tag: &str) -> (PathBuf, PathBuf) {
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "tinybrowser-logging-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{tag}.log"));
        (dir, path)
    }

    fn flush(tx: &SyncSender<Message>) {
        let (ack_tx, ack_rx) = mpsc::channel();
        tx.send(Message::Flush(ack_tx)).expect("flush");
        ack_rx.recv_timeout(TIMEOUT).expect("flush ack");
    }

    #[test]
    fn batches_lines_and_flushes_them() {
        let (dir, path) = temp_path("flush");
        let sink = FileSink::new(path.clone()).expect("sink");
        assert!(sink.try_send("one"));
        assert!(sink.try_send("two"));
        sink.flush();
        assert_eq!(fs::read_to_string(&path).expect("read"), "one\ntwo\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rotates_once_at_the_cap() {
        let (dir, path) = temp_path("rotate");
        let file = open(&path).expect("file");
        let (tx, rx) = mpsc::sync_channel(8);
        let writer_path = path.clone();
        let writer = thread::spawn(move || run(&rx, &writer_path, file, 16));

        assert!(tx.try_send(Message::Line("aaaaaaaaaa".to_owned())).is_ok());
        flush(&tx);
        assert!(tx.try_send(Message::Line("bbbbbbbbbb".to_owned())).is_ok());
        flush(&tx);
        assert!(tx.try_send(Message::Stop).is_ok());
        writer.join().expect("writer");

        assert_eq!(
            fs::read_to_string(backup_path(&path)).expect("backup"),
            "aaaaaaaaaa\n"
        );
        assert_eq!(fs::read_to_string(&path).expect("live"), "bbbbbbbbbb\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_queue_rejects_without_blocking() {
        let (sink, _never_read) = FileSink::stalled(1);
        assert!(sink.try_send("one"));
        assert!(!sink.try_send("two"));
    }
}
