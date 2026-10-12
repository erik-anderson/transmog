//! Bounded metadata envelopes. Keep payload JSON serialized until its record is requested.
use super::{BodyRef, CaptureError, CaptureLimits, Entry, StoredRecord};
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{fmt, marker::PhantomData};

// Normal writers flush near 128 KiB, well below this entry count. The separate
// ceiling prevents tiny malicious values from amplifying a 16 MiB leaf into a
// huge array of allocations before its first record is checked.
pub(super) const LEAF_ENTRIES: usize = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawEntry {
    pub index: u64,
    stored: Box<RawValue>,
    #[serde(default)]
    body: Option<BodyRef>,
}
impl RawEntry {
    pub fn decode(self, limits: CaptureLimits) -> Result<Entry, CaptureError> {
        check_record(self.stored.get().as_bytes(), limits)?;
        let stored: StoredRecord = serde_json::from_str(self.stored.get())?;
        Ok(Entry {
            index: self.index,
            stored,
            body: self.body,
        })
    }
}

// RawValue already validated the JSON syntax. This scan only estimates heap
// amplification: strings may grow buffers, arrays reserve Value slots, and even
// a tiny object allocates a full map node. Ignore punctuation inside strings.
// Apply the same bound when writing, before queueing the stored record.
pub(super) fn check_record(bytes: &[u8], limits: CaptureLimits) -> Result<(), CaptureError> {
    if bytes.len() > limits.max_record_bytes {
        return Err(CaptureError::RecordTooLarge {
            actual: bytes.len(),
            limit: limits.max_record_bytes,
        });
    }
    let maximum = limits
        .max_record_bytes
        .saturating_mul(16)
        .clamp(64 * 1024, 64 * 1024 * 1024);
    let mut estimate = bytes.len().saturating_mul(2);
    let mut quoted = false;
    let mut escaped = false;
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            estimate = estimate.saturating_add(match byte {
                b'"' => {
                    quoted = true;
                    0
                }
                b'{' => 1024,
                b':' => 184,
                b'[' => 128,
                b',' => 64,
                _ => 0,
            });
        }
        if estimate > maximum {
            return Err(CaptureError::QuotaExceeded);
        }
    }
    Ok(())
}

pub(super) fn array<T: DeserializeOwned>(
    bytes: &[u8],
    maximum: usize,
) -> Result<Vec<T>, CaptureError> {
    struct Bounded<T> {
        maximum: usize,
        marker: PhantomData<T>,
    }
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded metadata array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            // Never allocate from an untrusted size hint or declared count.
            let mut values = Vec::new();
            while values.len() < self.maximum {
                let Some(value) = sequence.next_element()? else {
                    return Ok(values);
                };
                values.push(value);
            }
            if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "metadata array exceeds its count limit",
                ));
            }
            Ok(values)
        }
    }
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let values = decoder.deserialize_seq(Bounded {
        maximum,
        marker: PhantomData,
    })?;
    decoder.end()?;
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrays_never_deserialize_values_beyond_their_limit() {
        struct Bomb;
        impl<'de> Deserialize<'de> for Bomb {
            fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
                panic!("an over-limit value must not be constructed");
            }
        }
        assert!(array::<Bomb>(b"[{}]", 0).is_err());
        assert!(array::<u8>(b"[1,2,3]", 2).is_err());
        assert!(array::<u8>(b"[1] []", 2).is_err());
        assert_eq!(array::<u8>(b"[1,2]", 2).unwrap(), vec![1, 2]);
    }

    #[test]
    fn raw_record_limit_precedes_payload_materialization() {
        // Valid JSON whose nesting is too deep to construct as a Value. The
        // byte quota must win before the typed payload deserializer sees it.
        let stored = format!("{}null{}", "[".repeat(256), "]".repeat(256));
        let envelope = format!(r#"{{"index":0,"stored":{stored}}}"#);
        let raw: RawEntry = serde_json::from_str(&envelope).unwrap();
        assert!(matches!(
            raw.decode(CaptureLimits {
                max_record_bytes: 128,
                ..CaptureLimits::default()
            }),
            Err(CaptureError::RecordTooLarge { .. })
        ));
    }

    #[test]
    fn tiny_object_amplification_is_rejected_before_constructing_values() {
        let payload = format!("[{}null]", r#"{"a":0},"#.repeat(60_000));
        let raw = format!(
            r#"{{"index":0,"stored":{{"revision":5,"sequence":0,"exchange_id":1,"kind":"extension","payload":{payload}}}}}"#
        );
        let entry: RawEntry = serde_json::from_str(&raw).unwrap();
        assert!(matches!(
            entry.decode(CaptureLimits::default()),
            Err(CaptureError::QuotaExceeded)
        ));
    }

    #[test]
    fn punctuation_and_escaped_quotes_inside_strings_do_not_charge_map_nodes() {
        let text = "{[: , \\\"}".repeat(10_000);
        let raw = serde_json::to_vec(&text).unwrap();
        check_record(
            &raw,
            CaptureLimits {
                max_record_bytes: raw.len(),
                ..CaptureLimits::default()
            },
        )
        .unwrap();
    }
}
