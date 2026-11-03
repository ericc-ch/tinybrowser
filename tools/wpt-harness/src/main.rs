//! In-process WPT testharness runner.
//!
//! Mounts each selected file in the real `renderer::Engine` without `WebDriver`,
//! wptserve, or a renderer child process. A paused Tokio clock jumps over
//! timer waits, so a hung test costs milliseconds instead of the harness
//! budget.
//!
//! The checkout is read-only. Requests are served from memory and the only
//! replacement is `/resources/testharnessreport.js`, which records the
//! completion payload the host polls.

mod cache;
mod deps;
mod engine;
mod report;
mod services;
mod suite;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use cache::Cache;
use engine::Outcome;
use suite::TestCase;

/// Parsed command line.
struct Options {
    root: PathBuf,
    paths: Vec<String>,
    jobs: usize,
    report: Option<PathBuf>,
    baseline: Option<PathBuf>,
    cache: Option<PathBuf>,
    deps: Option<PathBuf>,
    changed: Option<PathBuf>,
    verbose: bool,
    list: bool,
    limit: usize,
    offset: usize,
    mark_crashed: Option<String>,
    virtual_time: bool,
}

fn main() {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("wpt-harness: {message}");
            eprintln!("{HELP}");
            std::process::exit(2);
        }
    };
    if !options.root.is_dir() {
        eprintln!(
            "wpt-harness: {} is not a WPT checkout; pass --wpt-root",
            options.root.display()
        );
        std::process::exit(2);
    }
    if let Some(id) = &options.mark_crashed {
        let Some(path) = &options.cache else {
            eprintln!("wpt-harness: --mark-crashed needs --cache");
            exit(2);
        };
        let binary = match cache::binary_digest(Some(path)) {
            Ok(binary) => binary,
            Err(error) => {
                eprintln!("wpt-harness: cannot hash the harness binary: {error}");
                exit(2);
            }
        };
        let mut stored = match Cache::load(path, &binary) {
            Ok(cache) => cache,
            Err(error) => {
                eprintln!("wpt-harness: cannot read {}: {error}", path.display());
                exit(2);
            }
        };
        stored.mark_crashed(id, &options.root);
        if let Err(error) = stored.save(path) {
            eprintln!("wpt-harness: cannot write {}: {error}", path.display());
            exit(2);
        }
        exit(0);
    }
    let mut cases = match suite::select(&options.root, &options.paths, options.offset, options.limit) {
        Ok(cases) => cases,
        Err(error) => {
            eprintln!("wpt-harness: cannot read suite: {error}");
            std::process::exit(2);
        }
    };
    apply_selection(&options, &mut cases);
    if cases.is_empty() {
        eprintln!("wpt-harness: no tests selected");
        std::process::exit(2);
    }
    if options.list {
        for case in &cases {
            println!("{:?}\t{}", case.kind, case.relative);
        }
        return;
    }
    let started = Instant::now();
    let (outcomes, cached) = run_selected(&options, &cases);
    let wall = started.elapsed();
    summarize(&outcomes, wall, cached, options.verbose);
    if let Some(path) = &options.report
        && let Err(error) = report::write_report(path, &outcomes)
    {
        eprintln!("wpt-harness: cannot write {}: {error}", path.display());
        std::process::exit(2);
    }
    if let Some(baseline) = &options.baseline {
        match report::compare(baseline, &outcomes) {
            Ok(comparison) => {
                print_comparison(&comparison);
                if comparison.failed() {
                    exit(1);
                }
            }
            Err(error) => {
                eprintln!("wpt-harness: cannot compare {}: {error}", baseline.display());
                std::process::exit(2);
            }
        }
    }
    exit(0);
}

/// Leaves without running destructors.
///
/// A run's last engine can still root a `QuickJS` object, and freeing the
/// runtime then trips `QuickJS`'s own `gc_obj_list` assertion — after every
/// report is written. A browser process exits the same way, without walking
/// its heap, so the harness does not turn a finished run into a crash.
fn exit(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

/// Narrows `cases` to what the changed-file list selects, if a map is given.
fn apply_selection(options: &Options, cases: &mut Vec<TestCase>) {
    let (Some(map), Some(changed)) = (&options.deps, &options.changed) else {
        if options.deps.is_some() || options.changed.is_some() {
            eprintln!("wpt-harness: --deps and --changed go together");
            std::process::exit(2);
        }
        return;
    };
    match select_changed(map, changed) {
        Ok(Some(selected)) => {
            cases.retain(|case| selected.contains(&case.id()));
            eprintln!(
                "wpt-harness: dependency map selected {} tests",
                cases.len()
            );
        }
        Ok(None) => {
            eprintln!("wpt-harness: a changed file has no recorded test; running everything");
        }
        Err(error) => {
            eprintln!("wpt-harness: cannot select from the map: {error}");
            std::process::exit(2);
        }
    }
}

/// The tests a changed-file list selects, or `None` for the whole suite.
fn select_changed(
    map: &std::path::Path,
    changed: &std::path::Path,
) -> std::io::Result<Option<std::collections::BTreeSet<String>>> {
    let deps = deps::Deps::load(map)?;
    let text = std::fs::read_to_string(changed)?;
    let paths: Vec<String> = text
        .split(['\n', ','])
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.trim_start_matches("./").to_owned())
        .collect();
    let selection = deps.select(&paths);
    match &selection {
        Some(selected) => eprintln!(
            "wpt-harness: {} changed files select {} tests (map knows {} files)",
            paths.len(),
            selected.len(),
            deps.known().len()
        ),
        None => eprintln!(
            "wpt-harness: {} changed files include one the map never saw (map knows {} files)",
            paths.len(),
            deps.known().len()
        ),
    }
    Ok(selection)
}

/// Runs every case, reusing cached verdicts whose binary and inputs match.
///
/// Returns the outcomes in test order and how many came from the cache.
fn run_selected(options: &Options, cases: &[TestCase]) -> (Vec<Outcome>, usize) {
    let binary = match cache::binary_digest(options.cache.as_deref()) {
        Ok(binary) => binary,
        Err(error) => {
            eprintln!("wpt-harness: cannot hash the harness binary: {error}");
            std::process::exit(2);
        }
    };
    let mut stored = match &options.cache {
        Some(path) => match Cache::load(path, &binary) {
            Ok(cache) => Some(cache),
            Err(error) => {
                eprintln!("wpt-harness: cannot read {}: {error}", path.display());
                std::process::exit(2);
            }
        },
        None => None,
    };
    // A cached verdict is reused only when the binary and every file the test
    // read are unchanged, so a warm run costs hashing instead of execution.
    let mut slots: Vec<Option<Outcome>> = (0..cases.len()).map(|_| None).collect();
    let mut pending = Vec::new();
    if let Some(cache) = &stored {
        for (index, case) in cases.iter().enumerate() {
            match cache.lookup(&case.id(), &options.root) {
                Some(outcome) => slots[index] = Some(outcome),
                None => pending.push(index),
            }
        }
    } else {
        pending.extend(0..cases.len());
    }
    let cached = cases.len() - pending.len();
    if cached > 0 {
        eprintln!("wpt-harness: {cached} cached, {} to run", pending.len());
    }
    let selected: Vec<TestCase> = pending.iter().map(|index| cases[*index].clone()).collect();
    let fresh = run_all(
        &options.root,
        &selected,
        options.jobs,
        options.virtual_time,
    );
    for (index, outcome) in pending.into_iter().zip(fresh) {
        slots[index] = Some(outcome);
    }
    let outcomes: Vec<Outcome> = slots
        .into_iter()
        .enumerate()
        .map(|(index, outcome)| {
            outcome.unwrap_or_else(|| Outcome {
                id: cases[index].id(),
                kind: cases[index].kind,
                status: engine::Status::Skip,
                message: Some("worker stopped before this test".into()),
                subtests: Vec::new(),
                duration: std::time::Duration::ZERO,
                inputs: Vec::new(),
            })
        })
        .collect();
    if let Some(cache) = &mut stored {
        for outcome in &outcomes {
            cache.insert(outcome, &options.root);
        }
        if let Some(path) = &options.cache
            && let Err(error) = cache.save(path)
        {
            eprintln!("wpt-harness: cannot write {}: {error}", path.display());
            std::process::exit(2);
        }
    }
    (outcomes, cached)
}

/// Runs every case, one current-thread runtime per worker.
fn run_all(
    root: &std::path::Path,
    cases: &[TestCase],
    jobs: usize,
    virtual_time: bool,
) -> Vec<Outcome> {
    if cases.is_empty() {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    let workers = jobs.clamp(1, cases.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .start_paused(virtual_time)
                    .build()
                    .unwrap_or_else(|error| {
                        eprintln!("wpt-harness: cannot create runtime: {error}");
                        std::process::exit(2);
                    });
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(case) = cases.get(index) else {
                        break;
                    };
                    let started = Instant::now();
                    // One engine per file. Reusing one engine across a worker's
                    // files is faster, but a page can leave a `QuickJS` class
                    // cell borrowed, and the next mount then panics inside
                    // rquickjs. A file's own engine cannot see that state.
                    let mut session = engine::Session::new(root);
                    // Layout, paint, and the bindings can panic on a hostile
                    // page. A panic is this file's ERROR, not the end of the
                    // run; the engine that saw it is abandoned rather than
                    // dropped, because dropping a half-torn heap can panic
                    // again during unwinding.
                    let result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| {
                            runtime.block_on(session.run(case, virtual_time))
                        }),
                    );
                    let outcome = result.unwrap_or_else(|_| {
                        Box::leak(Box::new(session));
                        Outcome {
                            id: case.id(),
                            kind: case.kind,
                            status: engine::Status::Error,
                            message: Some("engine panicked".into()),
                            subtests: Vec::new(),
                            duration: started.elapsed(),
                            inputs: Vec::new(),
                        }
                    });                    if sender.send((index, outcome)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let mut slots: Vec<Option<Outcome>> = (0..cases.len()).map(|_| None).collect();
        for (index, outcome) in receiver {
            slots[index] = Some(outcome);
        }
        slots
            .into_iter()
            .enumerate()
            .map(|(index, outcome)| {
                outcome.unwrap_or_else(|| Outcome {
                    id: cases[index].id(),
                    kind: cases[index].kind,
                    status: engine::Status::Skip,
                    message: Some("worker stopped before this test".into()),
                    subtests: Vec::new(),
                    duration: std::time::Duration::ZERO,
                    inputs: Vec::new(),
                })
            })
            .collect()
    })
}

/// Prints counts and the slowest files.
fn summarize(outcomes: &[Outcome], wall: std::time::Duration, cached: usize, verbose: bool) {
    let mut pass = 0;
    let mut fail = 0;
    let mut timeout = 0;
    let mut error = 0;
    let mut skip = 0;
    let mut failures = Vec::new();
    let mut slowest: Vec<&Outcome> = outcomes.iter().collect();
    for outcome in outcomes {
        match outcome.status {
            engine::Status::Pass => pass += 1,
            engine::Status::Fail => {
                fail += 1;
                failures.push(outcome);
            }
            engine::Status::Timeout => timeout += 1,
            engine::Status::Error => error += 1,
            engine::Status::Skip => skip += 1,
        }
    }
    slowest.sort_by_key(|outcome| std::cmp::Reverse(outcome.duration));
    let count = u32::try_from(outcomes.len()).unwrap_or(u32::MAX);
    println!(
        "{} tests in {:.1?} ({:.2} ms/test): {pass} pass, {fail} fail, {timeout} timeout, \
         {error} error, {skip} skip, {cached} cached",
        outcomes.len(),
        wall,
        wall.as_secs_f64() * 1000.0 / f64::from(count)
    );
    if verbose {
        for outcome in failures {
            println!(
                "FAIL {} :: {}",
                outcome.id,
                outcome.message.as_deref().unwrap_or("")
            );
        }
        for outcome in slowest.iter().take(10) {
            println!("SLOW {:>8.1?} {}", outcome.duration, outcome.id);
        }
    }
}

/// Prints what changed against the baseline.
fn print_comparison(comparison: &report::Comparison) {
    for test in &comparison.regressions {
        println!("REGRESSION {test}");
    }
    for test in &comparison.new_failures {
        println!("NEW-FAILURE {test}");
    }
    for test in &comparison.fixes {
        println!("FIXED {test}");
    }
    println!(
        "baseline diff: {} regressions, {} new failures, {} fixes, {} tests not selected",
        comparison.regressions.len(),
        comparison.new_failures.len(),
        comparison.fixes.len(),
        comparison.missing.len()
    );
}

const HELP: &str = "\
usage: wpt-harness [OPTIONS] PATH...

Runs WPT testharness and crashtest files in-process.

  --wpt-root PATH   WPT checkout (default third_party/wpt)
  --jobs N          worker threads (default: available CPUs)
  --report PATH     write a wptreport JSON
  --baseline PATH   compare with an earlier --report and exit 1 on regressions
  --cache PATH      reuse unchanged verdicts from this cache file (and update it)
  --deps PATH       test-to-source map for --changed
  --changed PATH    one changed repository path per line (or comma separated)
  --limit N         stop after N selected files
  --offset N        skip the first N selected files (shards)
  --mark-crashed ID store an ERROR verdict for ID in --cache and exit
  --real-time       use the wall clock instead of the paused clock
  --verbose         print failing tests and the ten slowest files
  --list            print the selected files and exit
  -h, --help        show this help

PATH is relative to the WPT root, such as dom/nodes or dom/events/foo.html.
Reftests are selected but skipped until screenshot comparison lands.";

/// Parses the command line.
/// Takes the next argument as a path.
fn path_arg(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} needs a path"))
}

fn parse_args() -> Result<Options, String> {    let mut options = Options {
        root: suite::default_root(),
        paths: Vec::new(),
        jobs: std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        report: None,
        baseline: None,
        cache: None,
        deps: None,
        changed: None,
        verbose: false,
        list: false,
        limit: usize::MAX,
        offset: 0,
        mark_crashed: None,
        virtual_time: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--wpt-root" => options.root = path_arg(&mut args, "--wpt-root")?,
            "--jobs" => {
                options.jobs = args
                    .next()
                    .ok_or("--jobs needs a number")?
                    .parse()
                    .map_err(|_| "--jobs needs a number")?;
            }
            "--report" => options.report = Some(path_arg(&mut args, "--report")?),
            "--baseline" => options.baseline = Some(path_arg(&mut args, "--baseline")?),
            "--cache" => options.cache = Some(path_arg(&mut args, "--cache")?),
            "--deps" => options.deps = Some(path_arg(&mut args, "--deps")?),
            "--changed" => options.changed = Some(path_arg(&mut args, "--changed")?),
            "--limit" => {
                options.limit = args
                    .next()
                    .ok_or("--limit needs a number")?
                    .parse()
                    .map_err(|_| "--limit needs a number")?;
            }
            "--offset" => {
                options.offset = args
                    .next()
                    .ok_or("--offset needs a number")?
                    .parse()
                    .map_err(|_| "--offset needs a number")?;
            }
            "--mark-crashed" => {
                options.mark_crashed = Some(
                    args.next()
                        .ok_or("--mark-crashed needs a test id")?,
                );
            }
            "--real-time" => options.virtual_time = false,
            "--verbose" => options.verbose = true,
            "--list" => options.list = true,
            other if other.starts_with('-') => return Err(format!("unknown flag {other}")),
            other => options.paths.push(other.trim_start_matches('/').to_owned()),
        }
    }
    if options.paths.is_empty() && options.mark_crashed.is_none() {
        return Err("pass at least one test path".into());
    }
    Ok(options)
}
