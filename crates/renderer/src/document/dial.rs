use super::{CompletedDial, DialFail, QueuedDial};
use crate::protocol::{DialKind, DialOutcome, DialRequest};

pub(in crate::document) fn request(dial: &QueuedDial) -> DialRequest {
    let (url, kind, initiator) = match dial {
        QueuedDial::JsFetch { url, initiator, .. } => (url, DialKind::JsFetch, initiator),
        QueuedDial::ClassicScript { url, initiator, .. } => {
            (url, DialKind::ClassicScript, initiator)
        }
    };
    DialRequest {
        kind,
        url: url.to_string(),
        initiator: initiator.to_string(),
        read_body: true,
    }
}

pub(in crate::document) fn complete(
    dial: &QueuedDial,
    outcome: Option<DialOutcome>,
) -> Result<CompletedDial, DialFail> {
    let fail = match dial {
        QueuedDial::JsFetch { id, epoch, .. } => DialFail::JsFetch {
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => DialFail::ClassicScript { epoch: *epoch },
    };
    let outcome = outcome.ok_or(fail)?;
    Ok(match dial {
        QueuedDial::JsFetch { id, epoch, .. } => CompletedDial::JsFetch {
            status: outcome.status,
            body: outcome.body,
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => CompletedDial::ClassicScript {
            status: outcome.status,
            body: outcome.body,
            epoch: *epoch,
        },
    })
}

// https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm
// https://encoding.spec.whatwg.org/#concept-encoding-get
pub(in crate::document) fn decode_html(body: &[u8], content_type: Option<&str>) -> String {
    let bom = encoding_rs::Encoding::for_bom(body);
    let header = content_type.and_then(charset_from_content_type);
    let prescan = prescan_charset(body);
    let (encoding, bom_len) = bom
        .or_else(|| header.map(|encoding| (encoding, 0)))
        .or_else(|| prescan.map(|encoding| (encoding, 0)))
        .unwrap_or((encoding_rs::WINDOWS_1252, 0));
    encoding
        .decode_without_bom_handling(&body[bom_len..])
        .0
        .into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<&'static encoding_rs::Encoding> {
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
        let end = ascii[start..]
            .find('>')
            .map_or(ascii.len(), |offset| start + offset);
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
