//! The html5lib tree-construction conformance suite
//! (`third_party/html5lib-tests`, pinned per `third_party/VENDORED.md`).
//!
//! Every case feeds its `#data` markup through [`browser::parse_html`] (under
//! each scripting-flag setting the case demands) and compares the resulting
//! tree, rendered in html5lib dump form ([`dump`]), against the spec-mandated
//! `#document` section. This is the parse-correctness bar from
//! `docs/testing.md`: the same suite production engines run, covering the
//! tree-construction algorithms hand-written fixtures miss (foster parenting,
//! adoption agency, template contents).
//!
//! Gate policy (ADR 0005): a failing case either becomes a fix or a written
//! acceptance before this milestone closes — no silent ignores.

// Test-crate roots resolve `mod` against `tests/`, so the helpers live in
// their own directory next to this file, pinned explicitly.
#[path = "html5lib/dat.rs"]
mod dat;
#[path = "html5lib/dump.rs"]
mod dump;

use std::fs;
use std::path::{Path, PathBuf};

use browser::parse_html_with_scripting;

/// Cases where our tree is *known* to diverge from the suite's expectation
/// for reasons outside this workspace: html5ever itself leaves
/// `maybe_clone_an_option_into_selectedcontent` unimplemented ("will result
/// in a (slightly) incorrect DOM tree", per the trait docs), so an
/// `<option>` after `<selectedcontent>` under the relaxed `<select>` rules
/// mis-nests. Verified against rcdom (html5ever's own test DOM): it diverges
/// identically. See ADR 0005. Revisit whenever the html5ever pin moves.
const KNOWN_UPSTREAM_DIVERGENCES: &[(&str, &[usize], &str)] = &[(
    "webkit02.dat",
    &[44, 45, 46, 47],
    "html5ever: maybe_clone_an_option_into_selectedcontent unimplemented",
)];

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
    let mut deferred_fragment_cases = 0_usize;
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
            if case.fragment_context.is_some() {
                // Fragment parsing (`innerHTML`-style) is not exposed yet;
                // counted, not silently dropped. See VENDORED.md / ADR 0005.
                deferred_fragment_cases += 1;
                continue;
            }
            for &scripting_on in case.scripting.settings() {
                ran += 1;
                let parsed = parse_html_with_scripting(&case.data, scripting_on);
                let actual = dump::dump_document(&parsed.dom, &parsed.template_contents);
                if actual == case.document {
                    continue;
                }
                let known = KNOWN_UPSTREAM_DIVERGENCES
                    .iter()
                    .any(|(file, cases, _)| *file == file_name && cases.contains(&index));
                if known {
                    // Diverges exactly the way the documented upstream gap
                    // predicts; counted in the summary, never a failure.
                    accepted_divergences += 1;
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
         {accepted_divergences} runs matched documented upstream divergences, \
         {deferred_fragment_cases} deferred (#document-fragment cases await \
         fragment parsing)"
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
