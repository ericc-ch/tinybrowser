//! Value-only script results that may cross the browser seam.

use serde::{Deserialize, Serialize};

/// JSON-shaped script result. No DOM handles or `QuickJS` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RemoteValue {
    /// JS `undefined`.
    Undefined,
    /// JS `null`.
    Null,
    /// JS boolean.
    Bool(bool),
    /// JS number.
    Number(#[serde(with = "json_number")] f64),
    /// JS string.
    String(String),
    /// JS array.
    List(Vec<RemoteValue>),
    /// JS object as insertion-ordered entries.
    Map(Vec<(String, RemoteValue)>),
    /// Interned node identity allocated on the renderer.
    Node(u64),
}

/// JSON has no `NaN`/`Infinity`; plain `serde_json` silently writes `null`,
/// which then fails to deserialize and kills the renderer pipe. Encode
/// non-finite values as strings and accept both forms on the way back.
mod json_number {
    use serde::de::{self, Visitor};
    use serde::{Deserializer, Serializer};

    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde `with` serializers receive &T by contract"
    )]
    pub(super) fn serialize<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
        if value.is_finite() {
            serializer.serialize_f64(*value)
        } else if value.is_nan() {
            serializer.serialize_str("NaN")
        } else if value.is_sign_positive() {
            serializer.serialize_str("Infinity")
        } else {
            serializer.serialize_str("-Infinity")
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
        struct NumberVisitor;

        impl Visitor<'_> for NumberVisitor {
            type Value = f64;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a finite number or a NaN/Infinity string")
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<f64, E> {
                Ok(value)
            }

            #[allow(
                clippy::cast_precision_loss,
                reason = "JSON integers become JS f64 numbers"
            )]
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<f64, E> {
                Ok(value as f64)
            }

            #[allow(
                clippy::cast_precision_loss,
                reason = "JSON integers become JS f64 numbers"
            )]
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<f64, E> {
                Ok(value as f64)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<f64, E> {
                match value {
                    "NaN" => Ok(f64::NAN),
                    "Infinity" => Ok(f64::INFINITY),
                    "-Infinity" => Ok(f64::NEG_INFINITY),
                    _ => Err(E::custom("unknown non-finite number")),
                }
            }
        }

        deserializer.deserialize_any(NumberVisitor)
    }
}
