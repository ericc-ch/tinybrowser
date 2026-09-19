//! XML parsing for `DOMParser`'s XML MIME types.
//!
//! HTML documents are html5ever's job; XML documents need a scanner of
//! our own. It follows the XML 1.0 grammar for the constructs the DOM
//! can hold (elements, attributes, text, comments, CDATA, processing
//! instructions, doctype) and applies XML Namespaces scope rules so
//! parsed trees carry real namespace information: `xmlns`/`xmlns:*`
//! declarations stay in the tree as attributes in the XMLNS namespace,
//! and element and attribute qualified names resolve against the
//! declarations in scope. Malformed input yields a document holding a
//! `parsererror` element, matching the HTML XML parsing rules.

use dom::{
    Attribute, Dom, LocalName, Namespace, NodeId, Prefix, QualName, xml_namespace, xmlns_namespace,
};

use crate::Parsed;

/// Parses `input` as an XML document with the given content type.
pub(crate) fn parse_document(input: &str, content_type: &'static str) -> Parsed {
    let mut parser = XmlParser::new(input, content_type);
    if parser.run().is_err() {
        parser.malformed();
    }
    parser.finish()
}

struct XmlParser<'a> {
    input: &'a str,
    pos: usize,
    dom: Dom,
    document: NodeId,
    content_type: &'static str,
    /// Open elements, outermost first.
    stack: Vec<NodeId>,
    /// Namespace declarations introduced by each open element, parallel
    /// to `stack`; an empty-string prefix is the default declaration.
    scopes: Vec<Vec<(String, String)>>,
}

impl<'a> XmlParser<'a> {
    fn new(input: &'a str, content_type: &'static str) -> Self {
        let dom = Dom::new();
        let document = dom.document();
        Self {
            input,
            pos: 0,
            dom,
            document,
            content_type,
            stack: Vec::new(),
            scopes: Vec::new(),
        }
    }

    fn finish(self) -> Parsed {
        Parsed {
            dom: self.dom,
            quirks_mode: markup5ever::interface::QuirksMode::NoQuirks,
            content_type: self.content_type,
            ready_state: crate::ReadyState::Complete,
            url: None,
        }
    }

    /// Replaces everything parsed so far with a `parsererror` element.
    fn malformed(&mut self) {
        self.stack.clear();
        self.scopes.clear();
        let clear = self.dom.create_fragment();
        let _ = self.dom.replace_all(self.document, clear);
        let error = self.dom.create_element(
            QualName::new(
                None,
                Namespace::from("http://www.w3.org/1999/xhtml"),
                LocalName::from("parsererror"),
            ),
            Vec::new(),
        );
        let _ = self.dom.append(self.document, error);
    }

    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn skip(&mut self, bytes: usize) {
        self.pos += bytes;
    }

    fn starts_with(&self, needle: &str) -> bool {
        self.rest().starts_with(needle)
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start_matches(is_xml_whitespace).len();
    }

    /// Consumes an XML `Name`; `None` when the next characters cannot start
    /// one (<https://www.w3.org/TR/xml/#NT-Name>).
    fn take_name(&mut self) -> Option<&'a str> {
        let rest = self.rest();
        let mut end = 0;
        for (index, character) in rest.char_indices() {
            if index == 0 && !is_name_start(character) {
                return None;
            }
            if !is_name_char(character) {
                break;
            }
            end = index + character.len_utf8();
        }
        if end == 0 {
            return None;
        }
        let name = &rest[..end];
        self.pos += end;
        Some(name)
    }

    /// Consumes a quoted attribute or system/public literal, decoding
    /// character references in its value.
    fn take_quoted(&mut self) -> Option<String> {
        let quote = self.rest().chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        self.pos += 1;
        let rest = self.rest();
        let end = rest.find(quote)?;
        let raw = &rest[..end];
        self.pos += end + 1;
        let mut value = String::with_capacity(raw.len());
        decode_references(raw, true, &mut value).ok()?;
        Some(value)
    }

    fn append(&mut self, node: NodeId) -> Result<(), ()> {
        let parent = *self.stack.last().unwrap_or(&self.document);
        self.dom.append(parent, node).map_err(|_| ())
    }

    fn run(&mut self) -> Result<(), ()> {
        loop {
            let rest = self.rest();
            let Some(lt) = rest.find('<') else {
                if !rest.is_empty() {
                    self.push_text(rest)?;
                }
                return Ok(());
            };
            if lt > 0 {
                let text = &rest[..lt];
                self.pos += lt;
                self.push_text(text)?;
            }
            if self.starts_with("<!--") {
                self.comment()?;
            } else if self.starts_with("<![CDATA[") {
                self.cdata()?;
            } else if self.starts_with("<!DOCTYPE") {
                self.doctype()?;
            } else if self.starts_with("<!") {
                return Err(());
            } else if self.starts_with("<?") {
                self.processing_instruction()?;
            } else if self.starts_with("</") {
                self.end_tag()?;
            } else {
                self.start_tag()?;
            }
        }
    }

    fn push_text(&mut self, raw: &str) -> Result<(), ()> {
        if self.stack.is_empty() {
            // Only whitespace is allowed outside the document element
            // (<https://www.w3.org/TR/xml/#NT-Misc>).
            if raw.chars().all(is_xml_whitespace) {
                return Ok(());
            }
            return Err(());
        }
        let mut decoded = String::with_capacity(raw.len());
        decode_references(raw, false, &mut decoded)?;
        let node = self.dom.create_text(decoded);
        self.append(node)
    }

    fn comment(&mut self) -> Result<(), ()> {
        let rest = self.rest();
        let end = rest[4..].find("-->").ok_or(())? + 4;
        let node = self.dom.create_comment(&rest[4..end]);
        self.pos += end + 3;
        self.append(node)
    }

    fn cdata(&mut self) -> Result<(), ()> {
        let rest = self.rest();
        let end = rest[9..].find("]]>").ok_or(())? + 9;
        let node = self.dom.create_cdata_section(&rest[9..end]);
        self.pos += end + 3;
        self.append(node)
    }

    fn processing_instruction(&mut self) -> Result<(), ()> {
        let rest = self.rest();
        let end = rest[2..].find("?>").ok_or(())? + 2;
        let inner = &rest[2..end];
        let (target, data) = match inner.split_once(char::is_whitespace) {
            Some((target, data)) => (target, data.trim_start_matches(is_xml_whitespace)),
            None => (inner, ""),
        };
        if !is_valid_name(target) {
            return Err(());
        }
        self.pos += end + 2;
        if target.eq_ignore_ascii_case("xml") {
            return Ok(());
        }
        let node = self.dom.create_processing_instruction(target, data);
        self.append(node)
    }

    fn doctype(&mut self) -> Result<(), ()> {
        self.skip("<!DOCTYPE".len());
        self.skip_whitespace();
        let name = self.take_name().ok_or(())?.to_owned();
        self.skip_whitespace();
        let mut public_id = String::new();
        let mut system_id = String::new();
        if self.starts_with("PUBLIC") {
            self.skip("PUBLIC".len());
            self.skip_whitespace();
            public_id = self.take_quoted().ok_or(())?;
            self.skip_whitespace();
            if !self.starts_with(">") && !self.starts_with("[") {
                system_id = self.take_quoted().ok_or(())?;
                self.skip_whitespace();
            }
        } else if self.starts_with("SYSTEM") {
            self.skip("SYSTEM".len());
            self.skip_whitespace();
            system_id = self.take_quoted().ok_or(())?;
            self.skip_whitespace();
        }
        if self.starts_with("[") {
            self.skip_internal_subset()?;
            self.skip_whitespace();
        }
        if !self.starts_with(">") {
            return Err(());
        }
        self.skip(1);
        let node = self.dom.create_doctype(name, public_id, system_id);
        self.append(node)
    }

    fn skip_internal_subset(&mut self) -> Result<(), ()> {
        self.skip(1);
        let rest = self.rest();
        let mut quote = None;
        for (index, character) in rest.char_indices() {
            match (quote, character) {
                (Some(open), close) if close == open => quote = None,
                (None, '"' | '\'') => quote = Some(character),
                (None, ']') => {
                    self.pos += index + 1;
                    return Ok(());
                }
                _ => {}
            }
        }
        Err(())
    }

    fn end_tag(&mut self) -> Result<(), ()> {
        self.skip(2);
        let name = self.take_name().ok_or(())?;
        let Some(&top) = self.stack.last() else {
            return Err(());
        };
        let matches = matches!(
            self.dom.kind(top),
            Some(dom::NodeKind::Element { name: element, .. }) if qualified_equals(element, name)
        );
        if !matches {
            return Err(());
        }
        self.skip_whitespace();
        if !self.starts_with(">") {
            return Err(());
        }
        self.skip(1);
        self.stack.pop();
        self.scopes.pop();
        Ok(())
    }

    fn start_tag(&mut self) -> Result<(), ()> {
        self.skip(1);
        let name = self.take_name().ok_or(())?.to_owned();
        let (raw_attributes, self_closing) = self.parse_attributes()?;
        let declarations = namespace_declarations(&raw_attributes);
        let (element_name, attributes) = self.resolve_element(&name, &raw_attributes)?;
        let element = self.dom.create_element(element_name, attributes);
        self.append(element)?;
        if !self_closing {
            self.stack.push(element);
            self.scopes.push(declarations);
        }
        Ok(())
    }

    /// Consumes the attributes of a start tag, returning them in source
    /// order and whether the tag closes itself.
    fn parse_attributes(&mut self) -> Result<(Vec<(String, String)>, bool), ()> {
        let mut raw_attributes: Vec<(String, String)> = Vec::new();
        let self_closing = loop {
            self.skip_whitespace();
            if self.starts_with("/>") {
                self.skip(2);
                break true;
            }
            if self.starts_with(">") {
                self.skip(1);
                break false;
            }
            let attribute_name = self.take_name().ok_or(())?.to_owned();
            self.skip_whitespace();
            if !self.starts_with("=") {
                return Err(());
            }
            self.skip(1);
            self.skip_whitespace();
            let value = self.take_quoted().ok_or(())?;
            raw_attributes.push((attribute_name, value));
        };
        Ok((raw_attributes, self_closing))
    }

    /// Resolves an element's source name and attribute list against the
    /// namespace declarations in scope (including its own).
    fn resolve_element(
        &self,
        name: &str,
        raw_attributes: &[(String, String)],
    ) -> Result<(QualName, Vec<Attribute>), ()> {
        let (prefix, local) = split_name(name).ok_or(())?;
        if prefix == Some("xmlns") {
            return Err(());
        }
        let declarations = namespace_declarations(raw_attributes);
        let resolve = |wanted: &str| -> Option<&str> {
            declarations
                .iter()
                .rev()
                .find(|(declared, _)| declared == wanted)
                .map(|(_, value)| value.as_str())
                .or_else(|| {
                    self.scopes.iter().rev().find_map(|frame| {
                        frame
                            .iter()
                            .rev()
                            .find(|(declared, _)| declared == wanted)
                            .map(|(_, value)| value.as_str())
                    })
                })
        };
        let namespace = match prefix {
            Some("xml") => xml_namespace(),
            Some(other) => {
                let Some(uri) = resolve(other) else {
                    return Err(());
                };
                Namespace::from(uri)
            }
            None => Namespace::from(resolve("").unwrap_or("")),
        };

        let mut attributes = Vec::with_capacity(raw_attributes.len());
        for (raw_name, value) in raw_attributes {
            let attribute_name = if raw_name == "xmlns" {
                QualName::new(None, xmlns_namespace(), LocalName::from("xmlns"))
            } else if let Some(declared) = raw_name.strip_prefix("xmlns:") {
                if !is_valid_ncname(declared) {
                    return Err(());
                }
                QualName::new(
                    Some(Prefix::from("xmlns")),
                    xmlns_namespace(),
                    LocalName::from(declared),
                )
            } else if let Some(declared) = raw_name.strip_prefix("xml:") {
                if !is_valid_ncname(declared) {
                    return Err(());
                }
                QualName::new(
                    Some(Prefix::from("xml")),
                    xml_namespace(),
                    LocalName::from(declared),
                )
            } else if let Some((attribute_prefix, attribute_local)) = raw_name.split_once(':') {
                if !is_valid_ncname(attribute_prefix) || !is_valid_ncname(attribute_local) {
                    return Err(());
                }
                let Some(uri) = resolve(attribute_prefix) else {
                    return Err(());
                };
                QualName::new(
                    Some(Prefix::from(attribute_prefix)),
                    Namespace::from(uri),
                    LocalName::from(attribute_local),
                )
            } else {
                if !is_valid_ncname(raw_name) {
                    return Err(());
                }
                QualName::new(
                    None,
                    Namespace::from(""),
                    LocalName::from(raw_name.as_str()),
                )
            };
            attributes.push(Attribute {
                name: attribute_name,
                value: value.clone(),
            });
        }
        Ok((
            QualName::new(prefix.map(Prefix::from), namespace, LocalName::from(local)),
            attributes,
        ))
    }
}

/// Namespace declarations in an attribute list, as `(prefix, uri)` pairs.
/// The empty prefix is a default declaration.
fn namespace_declarations(attributes: &[(String, String)]) -> Vec<(String, String)> {
    let mut declarations = Vec::new();
    for (name, value) in attributes {
        if name == "xmlns" {
            declarations.push((String::new(), value.clone()));
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            declarations.push((prefix.to_owned(), value.clone()));
        }
    }
    declarations
}

/// Splits a source qualified name into prefix and local part, validating
/// both as XML names.
fn split_name(name: &str) -> Option<(Option<&str>, &str)> {
    if let Some((prefix, local)) = name.split_once(':') {
        if !is_valid_ncname(prefix) || !is_valid_ncname(local) {
            return None;
        }
        Some((Some(prefix), local))
    } else {
        if !is_valid_ncname(name) {
            return None;
        }
        Some((None, name))
    }
}

/// The source spelling of a qualified name, for end-tag matching.
fn qualified_equals(name: &QualName, source: &str) -> bool {
    match &name.prefix {
        Some(prefix) => source == format!("{prefix}:{}", name.local),
        None => source == name.local.as_ref(),
    }
}

/// Decodes the five predefined entities and numeric character references
/// (<https://www.w3.org/TR/xml/#sec-references>).
fn decode_references(raw: &str, in_attribute: bool, out: &mut String) -> Result<(), ()> {
    if in_attribute && raw.contains('<') {
        return Err(());
    }
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp + 1..];
        let semi = rest.find(';').ok_or(())?;
        let entity = &rest[..semi];
        match entity {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                let code = u32::from_str_radix(&entity[2..], 16).map_err(|_| ())?;
                out.push(char::from_u32(code).ok_or(())?);
            }
            _ if entity.starts_with('#') => {
                let code: u32 = entity[1..].parse().map_err(|_| ())?;
                out.push(char::from_u32(code).ok_or(())?);
            }
            _ => return Err(()),
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    Ok(())
}

fn is_xml_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r')
}

/// Whether `name` matches the XML `Name` production
/// (<https://www.w3.org/TR/xml/#NT-Name>).
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if is_name_start(first) => characters.all(is_name_char),
        _ => false,
    }
}

/// Whether `name` matches the XML `NCName` production (a `Name` without a
/// colon; <https://www.w3.org/TR/xml-names/#NT-NCName>).
pub(crate) fn is_valid_ncname(name: &str) -> bool {
    !name.contains(':') && is_valid_name(name)
}

// https://www.w3.org/TR/xml/#NT-NameStartChar
fn is_name_start(character: char) -> bool {
    matches!(character, ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
        || ('\u{10000}'..='\u{EFFFF}').contains(&character)
}

// https://www.w3.org/TR/xml/#NT-NameChar
fn is_name_char(character: char) -> bool {
    is_name_start(character)
        || matches!(character, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}
