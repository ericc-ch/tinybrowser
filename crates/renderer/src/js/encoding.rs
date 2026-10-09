//! Encoding Standard codecs for the JS shims.
//!
//! `encoding_rs` already owns labels, UTF-8, UTF-16, and the legacy tables.
//! The JS `TextEncoder` / `TextDecoder` surfaces stay in script; these host
//! functions are the crate calls those shims used to reimplement.
//!
//! <https://encoding.spec.whatwg.org/#interface-textdecoder>

use encoding_rs::{CoderResult, DecoderResult, Encoding};
use rquickjs::{Ctx, Exception, Object, Result, TypedArray, Value, prelude::Func};

/// Installs the encoding hooks the JS shim calls.
pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let host = super::bridge::object(ctx)?;
    host.set("__tbUtf8Encode", Func::from(utf8_encode))?;
    host.set("__tbEncodeInto", Func::from(encode_into))?;
    host.set("__tbDecode", Func::from(decode))?;
    Ok(())
}

/// UTF-8 encodes `input` after IDL string conversion, replacing unpaired
/// surrogates as the Encoding Standard's encoder does
/// (<https://encoding.spec.whatwg.org/#utf-8-encoder>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn utf8_encode<'js>(ctx: Ctx<'js>, input: Value<'js>) -> Result<TypedArray<'js, u8>> {
    let units = super::bindings::webidl_to_units(&ctx, input)?;
    let text = String::from_utf16_lossy(&units);
    let (bytes, _, _) = encoding_rs::UTF_8.encode(&text);
    TypedArray::new(ctx, bytes.into_owned())
}

/// UTF-8 encode-into: fill at most `max` bytes and report how many UTF-16
/// code units were consumed
/// (<https://encoding.spec.whatwg.org/#dom-textencoder-encodeinto>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn encode_into<'js>(ctx: Ctx<'js>, source: Value<'js>, max: usize) -> Result<Object<'js>> {
    let units = super::bindings::webidl_to_units(&ctx, source)?;
    let (read, written, bytes) = if max == 0 {
        (0, 0, Vec::new())
    } else {
        let mut dst = vec![0_u8; max];
        let mut encoder = encoding_rs::UTF_8.new_encoder();
        let (_, read, written, _) = encoder.encode_from_utf16(&units, &mut dst, true);
        dst.truncate(written);
        (read, written, dst)
    };
    let result = Object::new(ctx.clone())?;
    result.set("read", read)?;
    result.set("written", written)?;
    result.set("bytes", TypedArray::new(ctx, bytes)?)?;
    Ok(result)
}

/// Decodes isomorphic-latin1 `source` with Encoding Standard label `label`.
/// The result object has `text` and `remainder`; `stream` leaves a trailing
/// incomplete sequence in `remainder`
/// (<https://encoding.spec.whatwg.org/#concept-encoding-get>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes arguments by value"
)]
fn decode<'js>(
    ctx: Ctx<'js>,
    source: String,
    label: String,
    fatal: bool,
    ignore_bom: bool,
    stream: bool,
) -> Result<Object<'js>> {
    let bytes = bytes_from_latin1(&source)
        .ok_or_else(|| Exception::throw_type(&ctx, "The encoded data was not valid."))?;
    let Some(encoding) = Encoding::for_label(label.trim().as_bytes()) else {
        return Err(Exception::throw_range(
            &ctx,
            "The encoding label is not supported",
        ));
    };
    let (text, remainder) = decode_bytes(&bytes, encoding, fatal, ignore_bom, stream)
        .map_err(|_| Exception::throw_type(&ctx, "The encoded data was not valid."))?;
    let result = Object::new(ctx.clone())?;
    result.set("text", text)?;
    result.set("remainder", latin1_from_bytes(&remainder))?;
    Ok(result)
}

struct DecodeFailure;

fn decode_bytes(
    bytes: &[u8],
    encoding: &'static Encoding,
    fatal: bool,
    ignore_bom: bool,
    stream: bool,
) -> core::result::Result<(String, Vec<u8>), DecodeFailure> {
    let mut offset = 0;
    if !ignore_bom {
        if let Some((bom_encoding, bom_len)) = Encoding::for_bom(bytes) {
            if bom_encoding == encoding {
                offset = bom_len;
            }
        } else if stream && !bytes.is_empty() && is_bom_prefix_for(encoding, bytes) {
            return Ok((String::new(), bytes.to_vec()));
        }
    }
    let input = &bytes[offset..];
    let mut decoder = encoding.new_decoder_without_bom_handling();
    let last = !stream;
    let mut output = String::with_capacity(
        decoder
            .max_utf8_buffer_length(input.len())
            .unwrap_or(input.len().saturating_mul(3))
            .max(4),
    );
    let mut read = 0;
    loop {
        if fatal {
            let (result, consumed) =
                decoder.decode_to_string_without_replacement(&input[read..], &mut output, last);
            read += consumed;
            match result {
                DecoderResult::InputEmpty => break,
                DecoderResult::Malformed(_, _) => return Err(DecodeFailure),
                DecoderResult::OutputFull => {
                    reserve_decode(&mut decoder, &mut output, input.len().saturating_sub(read));
                }
            }
        } else {
            let (result, consumed, _) = decoder.decode_to_string(&input[read..], &mut output, last);
            read += consumed;
            match result {
                CoderResult::InputEmpty => break,
                CoderResult::OutputFull => {
                    reserve_decode(&mut decoder, &mut output, input.len().saturating_sub(read));
                }
            }
        }
    }
    Ok((output, input[read..].to_vec()))
}

fn reserve_decode(decoder: &mut encoding_rs::Decoder, output: &mut String, remaining: usize) {
    output.reserve(
        decoder
            .max_utf8_buffer_length(remaining)
            .unwrap_or(remaining.saturating_mul(3))
            .max(4),
    );
}

fn is_bom_prefix_for(encoding: &'static Encoding, bytes: &[u8]) -> bool {
    let bom: &[u8] = if encoding == encoding_rs::UTF_8 {
        &[0xEF, 0xBB, 0xBF]
    } else if encoding == encoding_rs::UTF_16LE {
        &[0xFF, 0xFE]
    } else if encoding == encoding_rs::UTF_16BE {
        &[0xFE, 0xFF]
    } else {
        return false;
    };
    bytes.len() < bom.len() && bom.starts_with(bytes)
}

/// Isomorphic decode: each code point below 256 becomes that byte
/// (<https://infra.spec.whatwg.org/#isomorphic-decode>).
fn bytes_from_latin1(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len());
    for character in text.chars() {
        bytes.push(u8::try_from(u32::from(character)).ok()?);
    }
    Some(bytes)
}

fn latin1_from_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| char::from(*byte)).collect()
}
