//! Export-only privacy projection; retained sessions and source files stay intact.
use transmog_capture::{CaptureRecordKind, CapturedHeader};

pub(crate) fn redact(kind: &mut CaptureRecordKind) {
    match kind {
        CaptureRecordKind::RequestHead { headers, .. }
        | CaptureRecordKind::ResponseHead { headers, .. }
        | CaptureRecordKind::Trailers { headers, .. } => {
            for field in headers {
                if sensitive(field)
                    && let Some(value) = field.value.take()
                {
                    field.original_value_bytes = Some(value.len());
                }
            }
        }
        CaptureRecordKind::Unknown { kind, payload } if kind == "entry-provenance" => {
            // Raw SAZ header text bypasses the structured headers. Omit it rather
            // than guessing at malformed/continued fields and leaking a value.
            if let Some(source) = payload
                .get_mut("source")
                .and_then(serde_json::Value::as_object_mut)
            {
                source.insert("rawHeaders".into(), serde_json::Value::Null);
                if let Some(notes) = source
                    .get_mut("diagnostics")
                    .and_then(serde_json::Value::as_array_mut)
                    && notes.len() < 64
                {
                    notes.push("Raw header text omitted by export redaction.".into());
                }
            }
        }
        _ => {}
    }
}
fn sensitive(field: &CapturedHeader) -> bool {
    [
        b"authorization".as_slice(),
        b"proxy-authorization",
        b"cookie",
        b"set-cookie",
    ]
    .iter()
    .any(|name| field.name.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn headers_trailers_and_raw_evidence_are_redacted_without_losing_size() {
        let mut kind = CaptureRecordKind::Trailers {
            boundary: transmog_core::observe::ExchangeBoundary::ClientResponse,
            headers: vec![CapturedHeader {
                name: b"Set-Cookie".to_vec(),
                value: Some(b"secret".to_vec()),
                original_value_bytes: None,
            }],
        };
        redact(&mut kind);
        let CaptureRecordKind::Trailers { headers, .. } = kind else {
            panic!()
        };
        assert_eq!(headers[0].value, None);
        assert_eq!(headers[0].original_value_bytes, Some(6));
        let mut raw = CaptureRecordKind::Unknown {
            kind: "entry-provenance".into(),
            payload: serde_json::json!({"source":{"rawHeaders":"Cookie: secret", "diagnostics":[]}}),
        };
        redact(&mut raw);
        let CaptureRecordKind::Unknown { payload, .. } = raw else {
            panic!()
        };
        assert!(!payload.to_string().contains("secret"));
    }
}
