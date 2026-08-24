//! Reader for html5lib tree-construction `.dat` files.
//!
//! Format per `tree-construction/README.md` upstream: a test is a run of
//! `#key`-prefixed sections; every `#data` line starts a new test, and each
//! section's value is everything up to the next `#key`, one newline per line.
//! The reference state machine is html5ever's own rcdom test driver.

use std::collections::HashMap;

/// Which [scripting-flag](https://html.spec.whatwg.org/multipage/parsing.html#scripting-flag)
/// settings a case must run under. Absent markers mean both, mirroring the
/// upstream README ("Otherwise, the test should be run in both modes").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scripting {
    /// Run twice: flag off, then flag on.
    Both,
    /// `#script-off` present: flag off only.
    DisabledOnly,
    /// `#script-on` present: flag on only.
    EnabledOnly,
}

impl Scripting {
    /// The concrete flag settings to run, in upstream's order (off, then on).
    pub fn settings(self) -> &'static [bool] {
        match self {
            Self::Both => &[false, true],
            Self::DisabledOnly => &[false],
            Self::EnabledOnly => &[true],
        }
    }
}

/// One parsed test case. Field values keep the file's exact bytes (minus the
/// section-separating trailing newline), because dump comparison is
/// whitespace-exact.
#[derive(Clone, Debug)]
pub struct Case {
    /// The `#data` section: markup fed to the parser.
    pub data: String,
    /// The `#document` section: the spec-mandated tree in html5lib dump form.
    pub document: String,
    /// The `#document-fragment` context element, if present. Such cases
    /// exercise fragment parsing, which `parse_html` does not offer.
    pub fragment_context: Option<String>,
    /// Which scripting-flag settings the case demands.
    pub scripting: Scripting,
}

/// Parses one whole `.dat` file into its cases, in file order.
pub fn parse_dat(content: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut fields: HashMap<String, String> = HashMap::new();
    // `None` until the first `#key`; `Some` while accumulating that key's value.
    let mut key: Option<String> = None;
    let mut value = String::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix('#') {
            if let Some(open) = key.take() {
                fields.insert(open, std::mem::take(&mut value));
            }
            if line == "#data" && !fields.is_empty() {
                cases.push(build_case(&fields));
                fields.clear();
            }
            key = Some(rest.to_owned());
        } else {
            value.push_str(line);
            value.push('\n');
        }
    }
    if let Some(open) = key.take() {
        fields.insert(open, value);
    }
    if !fields.is_empty() {
        cases.push(build_case(&fields));
    }
    cases
}

fn build_case(fields: &HashMap<String, String>) -> Case {
    let expect_field = |name: &str| {
        fields
            .get(name)
            .unwrap_or_else(|| panic!("test case is missing its #{name} section: {fields:?}"))
    };
    // Section values end with exactly one accumulated newline; the dump
    // comparison wants the content without it.
    let mut data = expect_field("data").clone();
    data.pop();
    let document = expect_field("document").trim_end_matches('\n').to_owned();
    let fragment_context = fields
        .get("document-fragment")
        .map(|context| context.trim_end_matches('\n').to_owned());
    let scripting = if fields.contains_key("script-off") {
        Scripting::DisabledOnly
    } else if fields.contains_key("script-on") {
        Scripting::EnabledOnly
    } else {
        Scripting::Both
    };
    Case {
        data,
        document,
        fragment_context,
        scripting,
    }
}
