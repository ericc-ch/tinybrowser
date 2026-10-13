//! HTML/XML response encoding sniffing (realm-free).
//!
//! The encoding-sniffing algorithm reads a byte-order mark, a transport
//! charset, an XML declaration, or an HTML prescan, in that order
//! (<https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm>,
//! <https://www.w3.org/TR/xml/#sec-guessing>). It is shared by the dial
//! response decoder and the XML response decoder in the JS bindings.

/// The response encoding from BOM, transport charset, declaration or
/// prescan, with the bytes already consumed
/// (<https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm>,
/// <https://encoding.spec.whatwg.org/#concept-encoding-get>).
pub(crate) fn sniff_encoding(
    bytes: &[u8],
    content_type: Option<&str>,
    eof: bool,
) -> Option<(&'static encoding_rs::Encoding, usize)> {
    if let Some(bom) = encoding_rs::Encoding::for_bom(bytes) {
        return Some(bom);
    }
    if !eof && bom_prefix(bytes) {
        return None;
    }
    if let Some(encoding) = content_type.and_then(charset_from_content_type) {
        return Some((encoding, 0));
    }
    // An XML document's encoding comes from the XML declaration, then UTF-8.
    // The HTML prescan and windows-1252 default do not apply
    // (<https://www.w3.org/TR/xml/#sec-guessing>,
    // <https://html.spec.whatwg.org/multipage/xhtml.html#parsing-xhtml-documents>).
    if content_type.is_some_and(|header| crate::xml::navigated_content_type(header).is_some()) {
        return xml_encoding(bytes, eof);
    }
    if let Some(encoding) = prescan_charset(bytes) {
        return Some((html_meta_encoding(encoding), 0));
    }
    (eof || bytes.len() >= 1024).then_some((encoding_rs::WINDOWS_1252, 0))
}

/// Labels from `<meta charset>` that the encoding spec names, remapped the
/// way HTML's "getting an encoding" step does
/// (<https://html.spec.whatwg.org/multipage/parsing.html#concept-encoding-get>).
fn html_meta_encoding(encoding: &'static encoding_rs::Encoding) -> &'static encoding_rs::Encoding {
    if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
        encoding_rs::UTF_8
    } else if encoding == encoding_rs::X_USER_DEFINED {
        encoding_rs::WINDOWS_1252
    } else {
        encoding
    }
}

/// XML encoding autodetection after a byte-order mark and a transport charset
/// have already been ruled out
/// (<https://www.w3.org/TR/xml/#sec-guessing>).
fn xml_encoding(bytes: &[u8], eof: bool) -> Option<(&'static encoding_rs::Encoding, usize)> {
    const MARK: &[u8] = b"<?xml";
    // UTF-16 without a BOM announces itself with null bytes. UCS-4 and
    // EBCDIC patterns have no codec here, so only the two UTF-16 patterns
    // are detected
    // (<https://www.w3.org/TR/xml/#sec-guessing>).
    const BE: &[u8] = &[0x00, 0x3C, 0x00, 0x3F];
    const LE: &[u8] = &[0x3C, 0x00, 0x3F, 0x00];
    if bytes.len() < 4
        && !eof
        && (MARK.starts_with(bytes) || BE.starts_with(bytes) || LE.starts_with(bytes))
    {
        return None;
    }
    if bytes.starts_with(BE) {
        return Some((encoding_rs::UTF_16BE, 0));
    }
    if bytes.starts_with(LE) {
        return Some((encoding_rs::UTF_16LE, 0));
    }
    if bytes.len() < MARK.len() && MARK.starts_with(bytes) && !eof {
        return None;
    }
    if !bytes.starts_with(MARK) {
        return Some((encoding_rs::UTF_8, 0));
    }
    let window = &bytes[..bytes.len().min(1024)];
    let Some(end) = window.windows(2).position(|pair| pair == b"?>") else {
        if !eof && bytes.len() < 1024 {
            return None;
        }
        return Some((encoding_rs::UTF_8, 0));
    };
    let declaration = &window[..end];
    Some((
        xml_encoding_attribute(declaration).unwrap_or(encoding_rs::UTF_8),
        0,
    ))
}

/// The `encoding` pseudo-attribute of an XML declaration, resolved as an
/// Encoding Standard label. Matches the token standalone on both sides so
/// `fooencoding=` or a quoted occurrence does not count
/// (<https://www.w3.org/TR/xml/#NT-EncodingDecl>).
///
/// Bounded: the declaration window is capped at 1024 bytes above, so the
/// byte-wise scan is at most ~1M comparisons per parse. Unknown labels fall
/// back to UTF-8 at the call site.
fn xml_encoding_attribute(declaration: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let token = b"encoding";
    let mut index = 0;
    while index + token.len() <= declaration.len() {
        if declaration[index..].starts_with(token) {
            let after = index + token.len();
            // The token must stand alone on both sides: `fooencoding=` and
            // `version="encoding"` are not the declaration. The declaration
            // always starts with `<?xml`, so index 0 cannot match.
            let preceded = index
                .checked_sub(1)
                .is_some_and(|before| declaration[before].is_ascii_whitespace());
            let continues = declaration.get(after).is_some_and(|byte| {
                byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b':')
            });
            if preceded && !continues {
                return encoding_label(&declaration[after..]);
            }
        }
        index += 1;
    }
    None
}

/// The encoding name after `encoding=`: optional whitespace, `=`, a
/// quoted label resolved as an Encoding Standard label
/// (<https://www.w3.org/TR/xml/#NT-EncodingDecl>).
fn encoding_label(rest: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let rest = rest.trim_ascii_start();
    let rest = rest.strip_prefix(b"=")?.trim_ascii_start();
    let quote = *rest.first()?;
    if quote != b'"' && quote != b'\'' {
        return None;
    }
    let value = rest.get(1..)?.split(|byte| *byte == quote).next()?;
    encoding_rs::Encoding::for_label(value)
}

fn bom_prefix(bytes: &[u8]) -> bool {
    [
        [0xEF, 0xBB, 0xBF].as_slice(),
        [0xFF, 0xFE].as_slice(),
        [0xFE, 0xFF].as_slice(),
    ]
    .iter()
    .any(|bom| bytes.len() < bom.len() && bom.starts_with(bytes))
}
/// The response charset: the MIME type's `charset` parameter resolved as an
/// Encoding Standard label
/// (<https://mimesniff.spec.whatwg.org/#parse-a-mime-type>,
/// <https://encoding.spec.whatwg.org/#concept-encoding-get>).
fn charset_from_content_type(content_type: &str) -> Option<&'static encoding_rs::Encoding> {
    if let Ok(mime) = content_type.parse::<mime::Mime>() {
        if let Some(charset) = mime.get_param(mime::CHARSET) {
            return encoding_rs::Encoding::for_label(charset.as_str().as_bytes());
        }
        return None;
    }
    // `mime` fails the whole parse on a parameter without `=`; the charset
    // after it is still usable, so fall back to a tolerant scan rather than
    // mis-decoding a labeled response.
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let label = value.trim().trim_matches(['\'', '"']);
        encoding_rs::Encoding::for_label(label.as_bytes())
    })
}

fn prescan_charset(body: &[u8]) -> Option<&'static encoding_rs::Encoding> {
    let prefix = &body[..body.len().min(1024)];
    let ascii = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    let mut cursor = 0;
    while let Some(relative_start) = ascii[cursor..].find("<meta") {
        let start = cursor + relative_start;
        // An unterminated tag can still be streaming: `<meta charset=utf-16`
        // would otherwise yield the valid label `utf-16` before the trailing
        // `be` arrives. Wait for `>` instead of using the buffer end.
        let Some(offset) = ascii[start..].find('>') else {
            break;
        };
        let end = start + offset;
        let tag = &ascii[start..end];
        if let Some(relative_charset) = tag.find("charset=") {
            let label = tag[relative_charset + "charset=".len()..]
                .trim_start_matches([' ', '\t', '\r', '\n', '\'', '"'])
                .split([' ', '\t', '\r', '\n', '\'', '"', ';', '>'])
                .next()?;
            if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                return Some(encoding);
            }
        }
        cursor = end.saturating_add(1);
    }
    None
}
