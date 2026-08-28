//! The html5lib tree-construction conformance suite
//! (`third_party/html5lib-tests`, pinned per `third_party/VENDORED.md`).
//!
//! Every case feeds its `#data` markup through [`browser::parse_html`] (under
//! each scripting-flag setting the case demands) and compares the resulting
//! tree, rendered in html5lib dump form ([`dump`]), against the spec-mandated
//! `#document` section. This is the parse-correctness bar from
//! `wiki/researches/testing.md`: the same suite production engines run, covering the
//! tree-construction algorithms hand-written fixtures miss (foster parenting,
//! adoption agency, template contents).
//!
//! Gate policy (ADR 0005): a failing case either becomes a fix or a written
//! acceptance before this milestone closes: no silent ignores.

// Test-crate roots resolve `mod` against `tests/`, so the helpers live in
// their own directory next to this file, pinned explicitly.
#[path = "html5lib/dat.rs"]
mod dat;
#[path = "html5lib/dump.rs"]
mod dump;

use std::fs;
use std::path::{Path, PathBuf};

use browser::{parse_html_fragment, parse_html_with_scripting};

/// html5ever calls `TreeSink::maybe_clone_an_option_into_selectedcontent` only
/// on an explicit `</option>` (servo/html5ever#712). We implement that hook.
/// `webkit02.dat` #44–47 omit `</option>`, so the builder never asks and
/// `<selectedcontent>` stays empty. `tests_innerHTML_1.dat` #75 is fragment
/// context `select` with data `<input><option>` (spec wants a lone `<option>`);
/// html5ever keeps `<input>` as well. Accepted dumps live in
/// `tests/html5lib/accepted/` so a worse tree than today's html5ever still
/// fails. See ADR 0005.
const KNOWN_UPSTREAM_DIVERGENCES: &[(&str, &[usize], &str)] = &[
    (
        "webkit02.dat",
        &[44, 45, 46, 47],
        "html5ever: option-clone hook skipped on implied </option> (servo/html5ever#712)",
    ),
    (
        "tests_innerHTML_1.dat",
        &[75],
        "html5ever: select fragment `<input><option>` keeps <input>; spec wants only <option>",
    ),
];

/// Locates the vendored suite, refusing to run silently when a fresh clone
/// skipped submodule initialization.
fn suite_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/html5lib-tests/tree-construction");
    assert!(
        dir.is_dir(),
        "html5lib-tests submodule is not initialized;\n\
         run `git submodule update --init` and retry\n\
         (expected suite at {})",
        dir.display()
    );
    dir
}

/// One spec divergence: enough context to diagnose without rerunning.
struct Failure {
    file: String,
    case_index: usize,
    scripting_on: bool,
    input: String,
    expected: String,
    actual: String,
}

#[test]
fn tree_construction_matches_spec_trees() {
    let dir = suite_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("suite directory is readable")
        .map(|entry| entry.expect("directory entry is readable").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dat"))
        .collect();
    // Deterministic order so reports read identically across runs.
    files.sort();

    let mut failures = Vec::new();
    let mut ran = 0_usize;
    let mut accepted_divergences = 0_usize;

    for path in &files {
        let file_name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("filename is UTF-8")
            .to_owned();
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {file_name}: {error}"));
        for (index, case) in dat::parse_dat(&content).into_iter().enumerate() {
            for &scripting_on in case.scripting.settings() {
                ran += 1;
                let actual = if let Some(context) = &case.fragment_context {
                    let parsed = parse_html_fragment(&case.data, context, scripting_on);
                    dump::dump_fragment(&parsed.dom)
                } else {
                    let parsed = parse_html_with_scripting(&case.data, scripting_on);
                    dump::dump_document(&parsed.dom)
                };
                if actual == case.document {
                    continue;
                }
                let known = KNOWN_UPSTREAM_DIVERGENCES
                    .iter()
                    .any(|(file, cases, _)| *file == file_name && cases.contains(&index));
                if known {
                    let pinned = accepted_dump(&file_name, index, scripting_on);
                    if pinned.as_deref() == Some(actual.as_str()) {
                        accepted_divergences += 1;
                    } else {
                        failures.push(Failure {
                            file: file_name.clone(),
                            case_index: index,
                            scripting_on,
                            input: case.data.clone(),
                            expected: pinned.unwrap_or_else(|| "(missing accepted dump)".into()),
                            actual,
                        });
                    }
                } else {
                    failures.push(Failure {
                        file: file_name.clone(),
                        case_index: index,
                        scripting_on,
                        input: case.data.clone(),
                        expected: case.document.clone(),
                        actual,
                    });
                }
            }
        }
    }

    println!(
        "html5lib tree-construction: {ran} cases run, \
         {accepted_divergences} runs matched documented upstream divergences"
    );
    assert!(
        failures.is_empty(),
        "{} of {ran} html5lib tree-construction cases diverge from the spec \
         tree:\n{}",
        failures.len(),
        report(&failures)
    );
}

/// Full diffs for the first few failures, then one-line summaries, so one
/// broken area does not bury the rest.
fn report(failures: &[Failure]) -> String {
    const FULL_DIFFS: usize = 5;
    let mut out = String::new();
    for failure in failures.iter().take(FULL_DIFFS) {
        let block = format!(
            "\n══ {} case #{} ({scripting}) ══\n\
             input:\n{input}\n\
             expected:\n{expected}\n\
             actual:\n{actual}\n",
            failure.file,
            failure.case_index,
            scripting = flag_name(failure.scripting_on),
            input = failure.input,
            expected = failure.expected,
            actual = failure.actual,
        );
        out.push_str(&block);
    }
    if failures.len() > FULL_DIFFS {
        let header = format!("\n… plus {} more:", failures.len() - FULL_DIFFS);
        out.push_str(&header);
        for failure in &failures[FULL_DIFFS..] {
            let line = format!(
                "\n  {} #{case} [{scripting}]",
                failure.file,
                case = failure.case_index,
                scripting = flag_name(failure.scripting_on),
            );
            out.push_str(&line);
        }
    }
    out
}

fn flag_name(scripting_on: bool) -> &'static str {
    if scripting_on {
        "script-on"
    } else {
        "script-off"
    }
}

fn accepted_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/html5lib/accepted")
}

fn accepted_path(file: &str, index: usize, scripting_on: bool) -> PathBuf {
    accepted_dir().join(format!("{file}.{index}.{}.dump", flag_name(scripting_on)))
}

fn accepted_dump(file: &str, index: usize, scripting_on: bool) -> Option<String> {
    fs::read_to_string(accepted_path(file, index, scripting_on)).ok()
}

/// Writes the current trees for [`KNOWN_UPSTREAM_DIVERGENCES`] into
/// `tests/html5lib/accepted/`. Run when bumping html5ever, not in CI.
#[test]
#[ignore = "writes accepted dumps; run after an html5ever pin change"]
fn write_accepted_upstream_dumps() {
    let dir = suite_dir();
    fs::create_dir_all(accepted_dir()).expect("accepted dir");
    for (file, cases, _) in KNOWN_UPSTREAM_DIVERGENCES {
        let content = fs::read_to_string(dir.join(file)).expect("dat readable");
        for (index, case) in dat::parse_dat(&content).into_iter().enumerate() {
            if !cases.contains(&index) {
                continue;
            }
            for &scripting_on in case.scripting.settings() {
                let actual = if let Some(context) = &case.fragment_context {
                    let parsed = parse_html_fragment(&case.data, context, scripting_on);
                    dump::dump_fragment(&parsed.dom)
                } else {
                    let parsed = parse_html_with_scripting(&case.data, scripting_on);
                    dump::dump_document(&parsed.dom)
                };
                fs::write(accepted_path(file, index, scripting_on), actual)
                    .expect("write accepted dump");
            }
        }
    }
}
