//! Per-file progress journal, so a process that dies mid-run resumes.
//!
//! Every file runs in one process, and a page can end it without unwinding:
//! a layout recursion overflows the stack, a `QuickJS` runtime aborts, a page
//! spins inside a drain that never returns to the harness loop. A panic is
//! one file's ERROR, but an abort used to be the whole chunk's.
//!
//! The journal records a line before a file starts and a line with its verdict
//! when it ends. The driver restarts the process after an abort, and the
//! journal says which files finished (report them again, do not run them) and
//! which were in flight (they get one file's ERROR or TIMEOUT). Lines are
//! written straight to the file rather than buffered, because the write that
//! matters most is the one immediately before an abort.

use std::collections::{BTreeSet, HashMap};
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use wpt_report::Outcome;

/// How a file a previous attempt left in flight is recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stale {
    /// The process died: a panic, an abort, or a signal.
    Died,
    /// The driver's watchdog cut a process that stopped returning.
    TimedOut,
}

/// One journal line.
#[derive(Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Record {
    /// The file is about to run.
    Start { id: String },
    /// The file has a verdict.
    Done { outcome: Outcome },
}

/// What an earlier attempt left behind.
#[derive(Default)]
pub struct Resumed {
    /// Verdicts the earlier attempt finished.
    pub done: HashMap<String, Outcome>,
    /// Files the earlier attempt started but never finished.
    pub stale: BTreeSet<String>,
}

/// The append-only progress log.
pub struct Journal {
    file: Mutex<File>,
}

impl Journal {
    /// Opens `path`, creating it when absent, and reads what an earlier attempt
    /// wrote.
    ///
    /// A line that does not parse is a torn write from the abort that ended
    /// the attempt: it is skipped, and the file it named (if any) runs again.
    ///
    /// # Errors
    ///
    /// The journal cannot be opened or read.
    pub fn open(path: &Path) -> io::Result<(Self, Resumed)> {
        let resumed = read(path)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        Ok((
            Self {
                file: Mutex::new(file),
            },
            resumed,
        ))
    }

    /// Records that `id` is about to run.
    pub fn start(&self, id: &str) {
        self.append(&Record::Start { id: id.to_owned() });
    }

    /// Records `outcome` as `id`'s verdict.
    pub fn finish(&self, outcome: &Outcome) {
        self.append(&Record::Done {
            outcome: outcome.clone(),
        });
    }

    /// Appends one line and hands it to the kernel.
    ///
    /// A write failure costs crash recovery, not the run: the verdicts already
    /// in memory still become the report. It is reported once per call so a
    /// full disk is visible in the log instead of silent.
    fn append(&self, record: &Record) {
        let Ok(mut line) = serde_json::to_vec(record) else {
            return;
        };
        line.push(b'\n');
        let mut file = match self.file.lock() {
            Ok(file) => file,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Err(error) = file.write_all(&line) {
            eprintln!("wpt-harness: cannot write the journal: {error}");
        }
    }
}

/// Reads the records an earlier attempt left.
fn read(path: &Path) -> io::Result<Resumed> {
    let mut resumed = Resumed::default();
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(resumed),
        Err(error) => return Err(error),
    };
    let mut started = BTreeSet::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match record {
            Record::Start { id } => {
                started.insert(id);
            }
            Record::Done { outcome } => {
                resumed.done.insert(outcome.id.clone(), outcome);
            }
        }
    }
    resumed.stale = started
        .into_iter()
        .filter(|id| !resumed.done.contains_key(id))
        .collect();
    Ok(resumed)
}

/// Why a stale file has no verdict.
#[must_use]
pub fn stale_message(stale: Stale) -> &'static str {
    match stale {
        Stale::Died => "the harness process died while this file ran",
        Stale::TimedOut => "the harness attempt timed out while this file ran",
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use wpt_report::{Outcome, Status};

    use super::{Journal, Stale, stale_message};

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wpt-harness-journal-{name}-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn outcome(id: &str, status: Status) -> Outcome {
        Outcome {
            id: id.into(),
            status,
            message: None,
            duration: Duration::from_millis(7),
            subtests: Vec::new(),
            testharness: true,
        }
    }

    /// A finished file is read back with its verdict and is not stale, so a
    /// resumed attempt reports it instead of running it again.
    #[test]
    fn a_finished_file_is_resumed_with_its_verdict() {
        let path = scratch("finished");
        {
            let (journal, resumed) = Journal::open(&path).expect("journal");
            assert!(resumed.done.is_empty());
            journal.start("/a.html");
            journal.finish(&outcome("/a.html", Status::Fail));
        }
        let (_, resumed) = Journal::open(&path).expect("reopened");
        assert_eq!(resumed.done["/a.html"].status, Status::Fail);
        assert_eq!(resumed.done["/a.html"].duration, Duration::from_millis(7));
        assert!(resumed.stale.is_empty());
        std::fs::remove_file(&path).expect("removed");
    }

    /// A file that started and never finished is stale exactly once: after the
    /// resumed attempt writes its verdict, a third attempt has no stale file.
    #[test]
    fn an_unfinished_file_is_stale_once() {
        let path = scratch("stale");
        {
            let (journal, _) = Journal::open(&path).expect("journal");
            journal.start("/b.html");
        }
        let (journal, resumed) = Journal::open(&path).expect("reopened");
        assert_eq!(
            resumed.stale.iter().cloned().collect::<Vec<_>>(),
            ["/b.html".to_owned()]
        );
        assert_eq!(
            stale_message(Stale::TimedOut),
            "the harness attempt timed out while this file ran"
        );
        journal.finish(&outcome("/b.html", Status::Timeout));
        drop(journal);
        let (_, resumed) = Journal::open(&path).expect("third attempt");
        assert!(resumed.stale.is_empty());
        assert_eq!(resumed.done["/b.html"].status, Status::Timeout);
        std::fs::remove_file(&path).expect("removed");
    }

    /// An abort can cut the last line in half; it names nothing, so that file
    /// runs again instead of inheriting a verdict nobody wrote.
    #[test]
    fn a_torn_line_is_ignored() {
        let path = scratch("torn");
        std::fs::write(
            &path,
            "{\"event\":\"start\",\"id\":\"/a.html\"}\n{\"event\":\"done\",\"outc",
        )
        .expect("written");
        let (_, resumed) = Journal::open(&path).expect("opened");
        assert_eq!(
            resumed.stale.iter().cloned().collect::<Vec<_>>(),
            ["/a.html".to_owned()]
        );
        assert!(resumed.done.is_empty());
        std::fs::remove_file(&path).expect("removed");
    }
}
