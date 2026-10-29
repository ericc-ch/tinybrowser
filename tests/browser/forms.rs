//! Form-control state on the paths that build elements from markup: the
//! sanitization algorithm, option selectedness, and activation dirtiness.

use serde_json::json;

use crate::fixture::{evaluate, ok, open_page, serve_on, server};

/// A blank page; every control is built by the script under test.
const PAGE: &[u8] = b"<!doctype html><meta charset=utf-8><body></body>";

/// A `value` attribute on a parser-created input runs the state's value
/// sanitization algorithm, whether the markup comes from `innerHTML`, a
/// template's contents, or an XML document
/// (<https://html.spec.whatwg.org/multipage/input.html#the-value-attribute>,
/// <https://html.spec.whatwg.org/multipage/input.html#the-input-element>).
const SANITIZE_FRAGMENT: &str = r#"(() => {
  const host = document.createElement('div');
  host.innerHTML = '<input id=n type=number value=abc>' +
    '<input id=r type=range>' +
    '<input id=c type=color>' +
    '<input id=d type=date value=nope>' +
    '<input id=t type=text value="a\nb">';
  document.body.append(host);
  const template = document.createElement('template');
  template.innerHTML = '<input id=tn type=number value=abc>';
  const xml = new DOMParser().parseFromString(
    '<root><input xmlns="http://www.w3.org/1999/xhtml" type="number" value="abc"/></root>',
    'application/xml',
  );
  return [
    host.querySelector('#n').value,
    host.querySelector('#r').value,
    host.querySelector('#c').value,
    host.querySelector('#d').value,
    host.querySelector('#t').value,
    template.content.querySelector('#tn').value,
    xml.querySelector('input').value,
  ];
})()"#;

/// Adding `selected` to an earlier option deselects the later one: a single
/// select keeps only the option the attribute just selected
/// (<https://html.spec.whatwg.org/multipage/form-elements.html#the-option-element>).
const SELECT_EARLIER_OPTION: &str = "(() => {
  const select = document.createElement('select');
  select.innerHTML = '<option id=a>A</option><option id=b selected>B</option>';
  document.body.append(select);
  const first = select.querySelector('#a');
  first.setAttribute('selected', '');
  return [select.selectedIndex, first.selected, select.querySelector('#b').selected];
})()";

/// A click sets the dirty checkedness flag, so a later `checked` attribute
/// write cannot restore the clicked state; a reset reseeds both slots from
/// the attributes
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked-dirty>,
/// <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
const ACTIVATION_AND_RESET: &str = "(() => {
  const form = document.createElement('form');
  form.innerHTML = '<input id=c type=checkbox checked><button id=b value=v>go</button>';
  document.body.append(form);
  const checkbox = form.querySelector('#c');
  checkbox.click();
  const afterClick = checkbox.checked;
  checkbox.removeAttribute('checked');
  checkbox.setAttribute('checked', '');
  const afterAttribute = checkbox.checked;
  form.reset();
  return [afterClick, afterAttribute, checkbox.checked, form.querySelector('#b').value];
})()";

#[test]
fn parser_created_controls_run_value_sanitization() {
    let (listener, page) = server();
    serve_on(listener, vec![("/", ok("text/html", PAGE))]);
    let (_fixture, mut client, session) =
        open_page("tinybrowser-forms-sanitize", &format!("http://{page}/"));

    let values = evaluate(&mut client, &session, SANITIZE_FRAGMENT);
    assert_eq!(
        values,
        json!(["", "50", "#000000", "", "ab", "", ""]),
        "every parser path reflects the sanitized value"
    );
}

#[test]
fn selected_attribute_change_keeps_one_option_selected() {
    let (listener, page) = server();
    serve_on(listener, vec![("/", ok("text/html", PAGE))]);
    let (_fixture, mut client, session) =
        open_page("tinybrowser-forms-option", &format!("http://{page}/"));

    let state = evaluate(&mut client, &session, SELECT_EARLIER_OPTION);
    assert_eq!(state, json!([0, true, false]));
}

#[test]
fn activation_dirties_checkedness_and_reset_reseeds_it() {
    let (listener, page) = server();
    serve_on(listener, vec![("/", ok("text/html", PAGE))]);
    let (_fixture, mut client, session) =
        open_page("tinybrowser-forms-activation", &format!("http://{page}/"));

    let state = evaluate(&mut client, &session, ACTIVATION_AND_RESET);
    assert_eq!(state, json!([false, false, true, "v"]));
}
