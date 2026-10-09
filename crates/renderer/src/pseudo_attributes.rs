//! Pseudo-attributes on a processing instruction.
//!
//! The DOM stores them as an ordered map and reserializes the whole map when
//! it changes
//! (<https://dom.spec.whatwg.org/#update-data-from-attributes>). Parsing
//! follows the XML stylesheet pseudo-attribute grammar
//! (<https://www.w3.org/TR/xml-stylesheet/#dt-parsing>). A parse error leaves
//! the map empty.
//!
//! Reserialization escapes `&`, then `<`, then `>`, then `"`. HTML
//! `outerHTML` does not escape `<` or `>` in attribute values, so a value
//! such as `axx>` or `some<>` will not match that test's `outerHTML` slice.
//! The processing-instruction algorithm is the one that applies here.

use crate::xml::{is_name_char, is_name_start};

/// Parses `data` into an ordered attribute list.
///
/// `None` is a parse error: the caller keeps an empty map
/// (<https://dom.spec.whatwg.org/#update-attributes-from-data>).
pub(crate) fn parse_pseudo_attributes(data: &str) -> Option<Vec<(String, String)>> {
    let mut chars = data.chars().peekable();
    let mut attributes = Vec::new();
    if chars.peek().is_none() {
        return Some(attributes);
    }
    loop {
        let name = parse_name(&mut chars)?;
        skip_space(&mut chars);
        if chars.next() != Some('=') {
            return None;
        }
        skip_space(&mut chars);
        let value = parse_value(&mut chars)?;
        if attributes.iter().any(|(existing, _)| existing == &name) {
            return None;
        }
        attributes.push((name, value));
        if chars.peek().is_none() {
            return Some(attributes);
        }
        if !take_space(&mut chars) {
            return None;
        }
        if chars.peek().is_none() {
            return Some(attributes);
        }
    }
}

/// Serializes an ordered attribute map
/// (<https://dom.spec.whatwg.org/#update-data-from-attributes>).
pub(crate) fn serialize_pseudo_attributes(attributes: &[(String, String)]) -> String {
    let mut data = String::new();
    for (index, (name, value)) in attributes.iter().enumerate() {
        if index > 0 {
            data.push(' ');
        }
        data.push_str(name);
        data.push('=');
        data.push('"');
        for character in value.chars() {
            match character {
                '&' => data.push_str("&amp;"),
                '<' => data.push_str("&lt;"),
                '>' => data.push_str("&gt;"),
                '"' => data.push_str("&quot;"),
                _ => data.push(character),
            }
        }
        data.push('"');
    }
    data
}

fn parse_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let first = chars.next().filter(|&character| is_name_start(character))?;
    let mut name = String::new();
    name.push(first);
    while chars
        .peek()
        .is_some_and(|character| is_name_char(*character))
    {
        name.push(chars.next()?);
    }
    Some(name)
}

fn parse_value(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut value = String::new();
    loop {
        match chars.next()? {
            character if character == quote => return Some(value),
            '<' => return None,
            '&' => value.push(parse_reference(chars)?),
            character if is_xml_char(character) => value.push(character),
            _ => return None,
        }
    }
}

fn parse_reference(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    let mut token = String::new();
    loop {
        match chars.next()? {
            ';' => break,
            character
                if token.len() < 16 && (character.is_ascii_alphanumeric() || character == '#') =>
            {
                token.push(character);
            }
            _ => return None,
        }
    }
    let character = match token.as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        other => {
            let code = if let Some(hex) = other
                .strip_prefix("#x")
                .or_else(|| other.strip_prefix("#X"))
            {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                let digits = other.strip_prefix('#')?;
                if digits.is_empty() || !digits.chars().all(|digit| digit.is_ascii_digit()) {
                    return None;
                }
                digits.parse().ok()?
            };
            char::from_u32(code).filter(|&character| is_xml_char(character))?
        }
    };
    Some(character)
}

fn skip_space(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while chars.peek().is_some_and(|character| is_space(*character)) {
        chars.next();
    }
}

fn take_space(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    if !chars.peek().is_some_and(|character| is_space(*character)) {
        return false;
    }
    skip_space(chars);
    true
}

/// XML whitespace (`S`), not ASCII whitespace: form feed is excluded
/// (<https://www.w3.org/TR/xml/#NT-S>).
fn is_space(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\r' | '\n')
}

/// XML `Char` (<https://www.w3.org/TR/xml/#NT-Char>).
fn is_xml_char(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{10FFFF}').contains(&character)
}
