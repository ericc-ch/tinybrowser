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
/// Longest a buffered record waits before the writer flushes it.
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
    /// Opens `path` (creating user-private parents) and starts the writer.
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

    /// Waits up to [`TIMEOUT`] for every accepted record to be written;
    /// `false` when the writer did not catch up in time.
    pub(crate) fn flush(&self) -> bool {
        let (ack_tx, ack_rx) = mpsc::channel();
        let mut message = Message::Flush(ack_tx);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match self.tx.try_send(message) {
                Ok(()) => return ack_rx.recv_timeout(TIMEOUT).is_ok(),
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    message = returned;
                    thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => return false,
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
///
/// The batch window starts when the first record enters the buffer, so a
/// steady trickle is written at least every [`BATCH_WINDOW`] instead of
/// resetting the timer on every record.
fn run(rx: &Receiver<Message>, path: &Path, mut file: File, rotate_at: u64) {
    let mut buffer = String::with_capacity(BATCH_BYTES);
    let mut records = 0_usize;
    let mut written = file.metadata().map_or(0, |meta| meta.len());
    let mut window_start = Instant::now();
    loop {
        let timeout = BATCH_WINDOW.saturating_sub(window_start.elapsed());
        match rx.recv_timeout(timeout) {
            Ok(Message::Line(line)) => {
                if records == 0 {
                    window_start = Instant::now();
                }
                buffer.push_str(line.trim_end_matches(['\r', '\n']));
                buffer.push('\n');
                records += 1;
                if records >= BATCH_RECORDS || buffer.len() >= BATCH_BYTES {
                    written = flush_buffer(
                        &mut file,
                        path,
                        &mut buffer,
                        &mut records,
                        written,
                        rotate_at,
                    );
                    window_start = Instant::now();
                }
            }
            Ok(Message::Flush(ack)) => {
                written = flush_buffer(
                    &mut file,
                    path,
                    &mut buffer,
                    &mut records,
                    written,
                    rotate_at,
                );
                window_start = Instant::now();
                let _ = ack.send(());
            }
            Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => {
                let _ = flush_buffer(
                    &mut file,
                    path,
                    &mut buffer,
                    &mut records,
                    written,
                    rotate_at,
                );
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                written = flush_buffer(
                    &mut file,
                    path,
                    &mut buffer,
                    &mut records,
                    written,
                    rotate_at,
                );
                window_start = Instant::now();
            }
        }
    }
}

fn flush_buffer(
    file: &mut File,
    path: &Path,
    buffer: &mut String,
    records: &mut usize,
    written: u64,
    rotate_at: u64,
) -> u64 {
    let written = write_batch(file, path, buffer, written, rotate_at);
    buffer.clear();
    *records = 0;
    written
}

/// Writes one batch, rotating first when it would cross the cap.
fn write_batch(file: &mut File, path: &Path, buffer: &str, written: u64, rotate_at: u64) -> u64 {
    if buffer.is_empty() {
        return written;
    }
    let Some(total) = written.checked_add(len(buffer)) else {
        return written;
    };
    if total > rotate_at {
        return rotate(file, path, buffer, written);
    }
    if file.write_all(buffer.as_bytes()).is_err() {
        return file.metadata().map_or(written, |meta| meta.len());
    }
    total
}

/// Renames the live file to `<file>.1` and starts a fresh one.
///
/// Every failure path keeps writing through the descriptor that stays open
/// and returns a count that matches the live file, so a failed rename cannot
/// silently disable the size cap:
/// - archive rename fails: append to the current file and keep counting;
/// - fresh file cannot be opened after a successful rename: rename the
///   archive back so the live path exists, then append through the old
///   descriptor.
fn rotate(file: &mut File, path: &Path, buffer: &str, written: u64) -> u64 {
    let backup = backup_path(path);
    let _ = fs::remove_file(&backup);
    let previous = written.saturating_add(len(buffer));
    if fs::rename(path, &backup).is_err() {
        return write_current(file, buffer, previous);
    }
    let mut options = File::options();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let fresh = options.open(path);
    if let Ok(fresh) = fresh {
        let _ = restrict_file(path);
        *file = fresh;
        write_current(file, buffer, len(buffer))
    } else {
        let _ = fs::rename(&backup, path);
        write_current(file, buffer, previous)
    }
}

/// Appends through the open descriptor; on failure the count resyncs from disk.
fn write_current(file: &mut File, buffer: &str, written: u64) -> u64 {
    if file.write_all(buffer.as_bytes()).is_err() {
        return file.metadata().map_or(written, |meta| meta.len());
    }
    written
}

fn len(buffer: &str) -> u64 {
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
        let _ = restrict_dir(parent);
    }
    let mut options = File::options();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    let _ = restrict_file(path);
    Ok(file)
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> io::Result<()> {
    Ok(())
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
        assert!(sink.flush());
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
    fn a_failed_archive_keeps_writing_and_counting() {
        let (dir, path) = temp_path("rotate-fail");
        let file = open(&path).expect("file");
        let (tx, rx) = mpsc::sync_channel(8);
        let writer_path = path.clone();
        let writer = thread::spawn(move || run(&rx, &writer_path, file, 16));

        assert!(tx.try_send(Message::Line("aaaaaaaaaa".to_owned())).is_ok());
        flush(&tx);
        // A non-empty directory at the backup path makes the archive rename fail.
        let backup = backup_path(&path);
        fs::create_dir(&backup).expect("backup dir");
        fs::write(backup.join("keep"), "x").expect("backup file");
        assert!(tx.try_send(Message::Line("bbbbbbbbbb".to_owned())).is_ok());
        flush(&tx);
        assert_eq!(
            fs::read_to_string(&path).expect("live"),
            "aaaaaaaaaa\nbbbbbbbbbb\n",
            "a failed archive must not lose records"
        );

        // Unblock rotation: the next over-cap batch archives both prior lines.
        fs::remove_dir_all(&backup).expect("unblock");
        assert!(tx.try_send(Message::Line("cccccccccc".to_owned())).is_ok());
        flush(&tx);
        assert!(tx.try_send(Message::Stop).is_ok());
        writer.join().expect("writer");
        assert_eq!(fs::read_to_string(&path).expect("live"), "cccccccccc\n");
        assert!(
            fs::read_to_string(&backup)
                .expect("backup")
                .contains("aaaaaaaaaa"),
            "archived batch missing"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_steady_trickle_flushes_within_the_window() {
        let (dir, path) = temp_path("trickle");
        let sink = FileSink::new(path.clone()).expect("sink");
        assert!(sink.try_send("one"));
        thread::sleep(Duration::from_millis(100));
        assert!(sink.try_send("two"));
        thread::sleep(Duration::from_millis(100));
        assert!(sink.try_send("three"));
        // The window started at the first record (~200 ms ago); a resetting
        // timer would not flush for another ~250 ms.
        thread::sleep(Duration::from_millis(150));
        let text = fs::read_to_string(&path).expect("read");
        assert!(text.contains("one"), "{text}");
        drop(sink);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_queue_rejects_without_blocking() {
        let (sink, _never_read) = FileSink::stalled(1);
        assert!(sink.try_send("one"));
        assert!(!sink.try_send("two"));
    }

    #[cfg(unix)]
    #[test]
    fn log_files_are_user_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, path) = temp_path("permissions");
        let sink = FileSink::new(path.clone()).expect("sink");
        assert!(sink.try_send("one"));
        assert!(sink.flush());
        let file_mode = fs::metadata(&path).expect("file").permissions().mode() & 0o777;
        let dir_mode = fs::metadata(&dir).expect("dir").permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "log file");
        assert_eq!(dir_mode, 0o700, "log directory");
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn rotated_log_files_are_user_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let (dir, path) = temp_path("rotated-permissions");
        let file = open(&path).expect("file");
        let (tx, rx) = mpsc::sync_channel(8);
        let writer_path = path.clone();
        let writer = thread::spawn(move || run(&rx, &writer_path, file, 1));

        assert!(tx.try_send(Message::Line("rotate".to_owned())).is_ok());
        flush(&tx);
        assert!(tx.try_send(Message::Stop).is_ok());
        writer.join().expect("writer");

        let mode = fs::metadata(&path).expect("live").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "rotated log file");
        let _ = fs::remove_dir_all(dir);
    }
}
