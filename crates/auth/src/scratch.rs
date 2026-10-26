//! Explicit, temporary credential-bearing JSON output; never used for inspection serialization.
use crate::{Context, MAX_CANDIDATE, Source, document::DecodedDocument, json::Node};
use std::sync::atomic::{AtomicBool, Ordering};

fn canceled(flag: &AtomicBool) -> Result<(), &'static str> {
    if flag.load(Ordering::Relaxed) {
        Err("Conversion canceled.")
    } else {
        Ok(())
    }
}

fn compact(raw: &str, flag: &AtomicBool) -> Result<DecodedDocument, &'static str> {
    canceled(flag)?;
    if raw.len() > MAX_CANDIDATE {
        return Err("Authentication input exceeds the 1 MiB token limit.");
    }
    let (value, _) = input_value(raw)?;
    let (token, _) = crate::jose::candidate(value);
    let mut context = Context::new(flag);
    if !context.charge(token.len()) {
        return Err("Authentication decoding exceeded its finite budget.");
    }
    let finding = crate::jose::decode(&mut context, token, Source::default())
        .ok_or("Expected a complete compact JWT or JOSE token with a JSON protected header.")?;
    context.push(finding);
    canceled(flag)?;
    if !context.result.diagnostics.is_empty() {
        return Err("Authentication decoding exceeded its finite budget.");
    }
    DecodedDocument::from_inspection(context.result)
        .pop()
        .ok_or("Authentication decoding produced no document.")
}

pub(crate) fn jwt(raw: &str, flag: &AtomicBool) -> Result<String, &'static str> {
    let document = compact(raw, flag)?;
    if document.format == "JWE" {
        return Err(
            "This is an encrypted JWE token. Its claims cannot be decoded without decryption.",
        );
    }
    if ["malformed", "limited"].contains(&document.status.as_str()) {
        return Err("JWT structure is malformed or exceeds the parser limits.");
    }
    let mut values = document.content;
    let header = values
        .iter()
        .find(|(name, _)| name == "header")
        .map(|(_, value)| value)
        .ok_or("JWT header is unavailable.")?;
    let alg = header
        .get("alg")
        .and_then(Node::string)
        .ok_or("JWT header requires one string alg member.")?;
    let signature = values
        .iter()
        .find(|(name, _)| name == "signature")
        .and_then(|(_, value)| value.string())
        .ok_or("JWT payload encoding is unsupported or malformed.")?;
    if signature.is_empty() != (alg == "none") {
        return Err("JWT signature is inconsistent with its algorithm.");
    }
    if !values
        .iter()
        .any(|(name, value)| name == "payload" && value.members().is_some())
    {
        return Err("JWT payload must be a JSON claims object within the depth and field limits.");
    }
    values.retain(|(name, _)| ["header", "payload", "signature"].contains(&name.as_str()));
    // Keep the existing converter output contract, independent of inspection presentation.
    values.sort_by_key(|(name, _)| match name.as_str() {
        "header" => 0,
        "payload" => 1,
        _ => 2,
    });
    values.push(("verification".into(), Node::Text("Not checked".into())));
    canceled(flag)?;
    Ok(Node::Object(values).pretty())
}

fn input_value(raw: &str) -> Result<(&str, Option<&str>), &'static str> {
    if raw.len() > MAX_CANDIDATE {
        return Err("Authentication input exceeds the 1 MiB token limit.");
    }
    let raw = raw.trim_ascii();
    if raw.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err("Paste one complete authentication header or token.");
    }
    if let Some((name, value)) = raw.split_once(':')
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        return Ok((value.trim_ascii(), Some(name)));
    }
    Ok((raw, None))
}

fn valid(document: &DecodedDocument) -> bool {
    document.complete
        && !["malformed", "limited"].contains(&document.status.as_str())
        && document.children.iter().all(valid)
}

fn documents(
    raw: &str,
    format: &str,
    flag: &AtomicBool,
) -> Result<Vec<DecodedDocument>, &'static str> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    canceled(flag)?;
    let (value, header) = input_value(raw)?;
    let mut context = Context::new(flag);
    if !context.charge(raw.len()) {
        return Err("Authentication decoding exceeded its finite budget.");
    }
    let source = Source {
        kind: "scratch".into(),
        label: header.unwrap_or("Pasted authentication input").into(),
        offset: Some(raw.len() - raw.trim_ascii_start().len()),
        length: Some(value.len()),
        ..Source::default()
    };
    let requested = match format {
        "basic-decode" => Some("Basic"),
        "digest-parse" => Some("Digest"),
        "ntlm-decode" => Some("NTLM"),
        "negotiate-decode" => Some("Negotiate"),
        _ => None,
    };
    if ["jws-decode", "jwe-inspect"].contains(&format) {
        return Ok(vec![compact(raw, flag)?]);
    }
    let info = header.is_some_and(|name| {
        name.eq_ignore_ascii_case("Authentication-Info")
            || name.eq_ignore_ascii_case("Proxy-Authentication-Info")
    });
    let first = value.split_ascii_whitespace().next().unwrap_or("");
    let known_scheme = ["Basic", "Digest", "Bearer", "NTLM", "Negotiate"]
        .iter()
        .any(|scheme| scheme.eq_ignore_ascii_case(first.trim_end_matches(',')));
    let header_envelope = header.is_some_and(|name| {
        [
            "Authorization",
            "Proxy-Authorization",
            "WWW-Authenticate",
            "Proxy-Authenticate",
        ]
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
    });
    let prefixed = if info {
        Some(format!("Digest {value}"))
    } else {
        requested
            .filter(|_| !known_scheme && !header_envelope)
            .map(|scheme| format!("{scheme} {value}"))
    };
    if known_scheme || header_envelope || prefixed.is_some() {
        let envelopes = crate::http::parse(prefixed.as_deref().unwrap_or(value));
        if envelopes.is_empty() {
            return Err("No supported authentication envelope was found.");
        }
        for envelope in envelopes {
            if requested.is_some_and(|scheme| !scheme.eq_ignore_ascii_case(&envelope.scheme)) {
                return Err("Input is not the requested authentication scheme.");
            }
            if format == "basic-decode" && envelope.token.is_none() {
                return Err("Basic decoding requires a complete Base64 credential.");
            }
            if format == "digest-parse" && envelope.token.is_some() {
                return Err("Digest parsing requires authentication parameters.");
            }
            let role = header.map_or("unknown", |name| {
                if name.eq_ignore_ascii_case("Authorization") {
                    "origin credential"
                } else if name.eq_ignore_ascii_case("Proxy-Authorization") {
                    "proxy credential"
                } else if name.eq_ignore_ascii_case("WWW-Authenticate") {
                    "origin challenge"
                } else if name.eq_ignore_ascii_case("Proxy-Authenticate") {
                    "proxy challenge"
                } else {
                    "unknown"
                }
            });
            let mut finding = crate::http::decode(&mut context, envelope, source.clone(), role);
            if prefixed.is_some() {
                finding.source.offset = None;
                finding.source.length = None;
                finding.source.transforms.push(
                    if info {
                        "Authentication-Info parameters; original source has no scheme"
                    } else {
                        "Explicit format interpretation of a bare token or parameter list"
                    }
                    .into(),
                );
            }
            context.push(finding);
        }
    } else if let Some(finding) = crate::jose::decode(&mut context, value, source.clone()) {
        context.push(finding);
    } else if let Ok(bytes) = STANDARD.decode(value) {
        if !(bytes.starts_with(b"NTLMSSP\0")
            || bytes
                .first()
                .is_some_and(|tag| [0x60, 0xa0, 0xa1, 0x6e, 0x6f, 0x7e].contains(tag)))
        {
            return Err(
                "Input has no recognizable authentication structure. Choose an explicit format for bare Base64.",
            );
        }
        if !context.charge(bytes.len()) {
            return Err("Authentication decoding exceeded its finite budget.");
        }
        let finding = crate::windows::decode(
            &mut context,
            &bytes,
            source,
            bytes.starts_with(b"NTLMSSP\0"),
        );
        context.push(finding);
    } else {
        return Err(
            "Input has no recognizable authentication structure. Choose an explicit format for bare Base64.",
        );
    }
    canceled(flag)?;
    if !context.result.diagnostics.is_empty() {
        return Err("Authentication decoding exceeded its finite budget.");
    }
    Ok(DecodedDocument::from_inspection(context.result))
}

/// Decode temporary scratch authentication material without verification or network access.
/// The format is an explicit operation ID; errors never contain credential material.
pub fn decode_authentication(
    raw: &str,
    format: &str,
    flag: &AtomicBool,
) -> Result<String, &'static str> {
    if !crate::AUTH_CONVERSIONS
        .iter()
        .any(|operation| operation.id == format)
    {
        return Err("Unsupported authentication conversion.");
    }
    if format == "jwt-decode" {
        return jwt(raw, flag);
    }
    let documents = documents(raw, format, flag)?;
    if documents.is_empty() || !documents.iter().all(valid) {
        return Err(
            "Authentication structure is malformed, incomplete or exceeds the parser limits.",
        );
    }
    if let Some(document) = documents.first()
        && ((format == "jwe-inspect" && document.format != "JWE")
            || (format == "jws-decode" && document.format == "JWE"))
    {
        return Err("Input is not the requested JOSE format.");
    }
    let result = if documents.len() == 1 {
        documents
            .into_iter()
            .next()
            .ok_or("Authentication decoding produced no document.")?
            .scratch()
    } else {
        Node::Object(vec![
            ("format".into(), Node::Text("HTTP authentication".into())),
            (
                "artifacts".into(),
                Node::Array(
                    documents
                        .into_iter()
                        .map(DecodedDocument::scratch)
                        .collect(),
                ),
            ),
        ])
    }
    .pretty();
    canceled(flag)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    fn run(input: &str, operation: &str) -> serde_json::Value {
        serde_json::from_str(
            &decode_authentication(input, operation, &AtomicBool::new(false)).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn paste_envelopes_parameters_unknown_roles_and_opaque_values() {
        let basic = run("Authorization: Basic dXNlcjpwYXNzOm1vcmU=", "auth-decode");
        assert_eq!(basic["format"], "Basic");
        assert_eq!(basic["decoded"]["passwordBytesBase64"], "cGFzczptb3Jl");
        assert_eq!(
            run("dXNlcjpwYXNzOm1vcmU=", "basic-decode")["role"],
            "unknown"
        );
        assert!(
            decode_authentication("dXNlcjpwYXNz", "auth-decode", &AtomicBool::new(false)).is_err()
        );
        let digest = run(
            "WWW-Authenticate: Digest realm=\"a,b\", nonce=\"one\", nonce=\"two\", Basic realm=\"other\"",
            "auth-decode",
        );
        assert_eq!(
            digest["artifacts"][0]["decoded"]["parameters"][2]["value"],
            "two"
        );
        assert_eq!(digest["artifacts"][0]["status"], "incomplete");
        assert_eq!(digest["artifacts"][1]["format"], "Basic");
        let info = run(
            "Proxy-Authentication-Info: rspauth=\"secret\", qop=auth",
            "auth-decode",
        );
        assert_eq!(info["decoded"]["parameters"][0]["name"], "rspauth");
        assert_eq!(info["role"], "unknown");
        assert_eq!(
            run("Bearer opaque-token", "auth-decode")["status"],
            "opaque"
        );
        assert_eq!(
            run("Authorization: Custom opaque-token", "auth-decode")["status"],
            "unsupported"
        );
        assert_eq!(
            run("realm=\"standalone\", nonce=\"n\"", "digest-parse")["role"],
            "unknown"
        );
        let non_utf8 = run(&STANDARD.encode([0xff, b':', 0x81]), "basic-decode");
        assert_eq!(non_utf8["decoded"]["usernameBytesBase64"], "/w==");
        assert!(!non_utf8.to_string().contains('\u{fffd}'));
    }
    #[test]
    fn encrypted_and_invalid_pastes_are_never_complete_decodes() {
        let encrypted = "eyJhbGciOiJkaXIiLCJlbmMiOiJBMTI4R0NNIn0..aXY.Y2lwaGVy.dGFn";
        assert_eq!(run(encrypted, "jwe-inspect")["status"], "encrypted");
        assert_eq!(
            run(&format!("X-Token: {encrypted}"), "auth-decode")["format"],
            "JWE"
        );
        let flag = AtomicBool::new(false);
        for (text, operation) in [
            ("Basic !!!private-marker", "auth-decode"),
            ("Bearer abc ! trailing", "auth-decode"),
            ("Digest realm=\"unterminated", "auth-decode"),
            ("Digest realm=a, @@@", "auth-decode"),
            ("Authorization: Basic Zm9v", "basic-decode"),
            ("Authorization: Digest realm=x", "basic-decode"),
            ("Authorization: Bearer x\nOther: y", "auth-decode"),
            ("NTLM TlRMTVNTUAA=", "ntlm-decode"),
            (encrypted, "jws-decode"),
        ] {
            assert!(
                decode_authentication(text, operation, &flag).is_err(),
                "{text}"
            );
        }
        let many = (0..65)
            .map(|_| "Basic realm=x")
            .collect::<Vec<_>>()
            .join(", ");
        assert!(decode_authentication(&many, "auth-decode", &flag).is_err());
        assert!(
            decode_authentication(&"x".repeat(MAX_CANDIDATE + 1), "auth-decode", &flag).is_err()
        );
        assert!(
            decode_authentication("Bearer secret", "auth-decode", &AtomicBool::new(true)).is_err()
        );
    }
}
