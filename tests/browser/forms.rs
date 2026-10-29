//! Form-control state on the paths that build controls from markup: the
//! value sanitization algorithm, option selectedness, and activation
//! dirtiness.

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
/// write cannot restore the clicked state; a reset reseeds the slot from the
/// attributes
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked-dirty>,
/// <https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#concept-form-reset-control>).
const ACTIVATION_AND_RESET: &str = "(() => {
  const form = document.createElement('form');
  form.innerHTML = '<input id=c type=checkbox checked>';
  document.body.append(form);
  const checkbox = form.querySelector('#c');
  checkbox.click();
  const afterClick = checkbox.checked;
  checkbox.removeAttribute('checked');
  checkbox.setAttribute('checked', '');
  const afterAttribute = checkbox.checked;
  form.reset();
  return [afterClick, afterAttribute, checkbox.checked];
})()";

/// A checkbox that script checks before its first layout keeps that
/// checkedness when the widget is built: layout reads the slot instead of
/// re-seeding it from the `checked` attribute
/// (<https://html.spec.whatwg.org/multipage/input.html#concept-input-checked>).
const CHECKED_BEFORE_LAYOUT: &str = "(() => {
  const box = document.createElement('input');
  box.type = 'checkbox';
  box.checked = true;
  document.body.append(box);
  // A layout metric read builds the box, including the checkbox widget.
  void box.offsetHeight;
  const afterLayout = box.checked;
  const matchesChecked = box.matches(':checked');
  // The click's dirty flag still wins over the attribute.
  box.setAttribute('checked', '');
  return [afterLayout, matchesChecked, box.checked, box.matches(':checked')];
})()";

/// Two checked radios inserted together leave the last one checked: the
/// insertion step runs per control and clears the group members, so an
/// earlier insertion must not clear a radio the batch has not processed yet
/// (<https://html.spec.whatwg.org/multipage/input.html#radio-button-group>).
const TWO_CHECKED_RADIOS: &str = "(() => {
  const form = document.createElement('form');
  form.innerHTML = '<input id=a type=radio name=r checked>' +
    '<input id=b type=radio name=r checked>';
  document.body.append(form);
  return [form.querySelector('#a').checked, form.querySelector('#b').checked];
})()";

#[test]
fn control_state_follows_markup_insertion_and_activation() {
    let (listener, page) = server();
    serve_on(listener, vec![("/", ok("text/html", PAGE))]);
    let (_fixture, mut client, session) =
        open_page("tinybrowser-forms", &format!("http://{page}/"));

    let values = evaluate(&mut client, &session, SANITIZE_FRAGMENT);
    assert_eq!(
        values,
        json!(["", "50", "#000000", "", "ab", "", ""]),
        "every parser path reflects the sanitized value"
    );

    let state = evaluate(&mut client, &session, SELECT_EARLIER_OPTION);
    assert_eq!(state, json!([0, true, false]));

    let state = evaluate(&mut client, &session, ACTIVATION_AND_RESET);
    assert_eq!(state, json!([false, false, true]));

    let state = evaluate(&mut client, &session, CHECKED_BEFORE_LAYOUT);
    assert_eq!(state, json!([true, true, true, true]));

    let state = evaluate(&mut client, &session, TWO_CHECKED_RADIOS);
    assert_eq!(state, json!([false, true]));
}
