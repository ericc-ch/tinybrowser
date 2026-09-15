use super::{CompletedDial, DialFail, QueuedDial};
use crate::protocol::{DialKind, DialOutcome, DialRequest};

pub(crate) struct ResponseDecoder {
    content_type: Option<String>,
    pending: Vec<u8>,
    decoder: Option<encoding_rs::Decoder>,
}

impl ResponseDecoder {
    pub(crate) fn new(content_type: Option<String>) -> Self {
        Self {
            content_type,
            pending: Vec::new(),
            decoder: None,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
        if let Some(decoder) = &mut self.decoder {
            return decode_chunk(decoder, bytes, false);
        }
        self.pending.extend_from_slice(bytes);
        let Some((encoding, bom_len)) =
            sniff_encoding(&self.pending, self.content_type.as_deref(), false)
        else {
            return String::new();
        };
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let output = decode_chunk(&mut decoder, &self.pending[bom_len..], false);
        self.pending.clear();
        self.decoder = Some(decoder);
        output
    }

    pub(crate) fn finish(mut self) -> String {
        if let Some(decoder) = &mut self.decoder {
            return decode_chunk(decoder, &[], true);
        }
        let (encoding, bom_len) = sniff_encoding(&self.pending, self.content_type.as_deref(), true)
            .unwrap_or((encoding_rs::WINDOWS_1252, 0));
        let mut decoder = encoding.new_decoder_without_bom_handling();
        decode_chunk(&mut decoder, &self.pending[bom_len..], true)
    }
}

fn decode_chunk(decoder: &mut encoding_rs::Decoder, bytes: &[u8], last: bool) -> String {
    let mut output = String::with_capacity(
        decoder
            .max_utf8_buffer_length(bytes.len())
            .unwrap_or(bytes.len().saturating_mul(3))
            .max(4),
    );
    let mut read = 0;
    loop {
        let (result, consumed, _) = decoder.decode_to_string(&bytes[read..], &mut output, last);
        read += consumed;
        match result {
            encoding_rs::CoderResult::InputEmpty => return output,
            encoding_rs::CoderResult::OutputFull => {
                let remaining = bytes.len().saturating_sub(read);
                output.reserve(
                    decoder
                        .max_utf8_buffer_length(remaining)
                        .unwrap_or(remaining.saturating_mul(3))
                        .max(4),
                );
            }
        }
    }
}

// https://html.spec.whatwg.org/multipage/parsing.html#encoding-sniffing-algorithm
// https://encoding.spec.whatwg.org/#concept-encoding-get
fn sniff_encoding(
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
    if let Some(encoding) = prescan_charset(bytes) {
        return Some((encoding, 0));
    }
    (eof || bytes.len() >= 1024).then_some((encoding_rs::WINDOWS_1252, 0))
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
    outcome: Result<DialOutcome, crate::protocol::DialFailure>,
) -> Result<CompletedDial, DialFail> {
    let fail = match dial {
        QueuedDial::JsFetch { id, epoch, .. } => DialFail::JsFetch {
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { epoch, .. } => DialFail::ClassicScript { epoch: *epoch },
    };
    let outcome = outcome.map_err(|_| fail)?;
    Ok(match dial {
        QueuedDial::JsFetch { id, epoch, .. } => CompletedDial::JsFetch {
            status: outcome.status,
            body: outcome.body,
            id: *id,
            epoch: *epoch,
        },
        QueuedDial::ClassicScript { element, epoch, .. } => CompletedDial::ClassicScript {
            status: outcome.status,
            body: outcome.body,
            element: *element,
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
