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
    let (token, _) = crate::jose::candidate(raw);
    let mut context = Context::new(flag);
    if !context.charge(token.len()) {
        return Err("Authentication decoding exceeded its finite budget.");
    }
    let finding = crate::jose::decode(&mut context, token, Source::default())
        .ok_or("Expected a complete compact JOSE token with a JSON protected header.")?;
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

/// Decode temporary scratch authentication material without verification or network access.
/// The format is an explicit operation ID; errors never contain credential material.
pub fn decode_authentication(
    raw: &str,
    format: &str,
    flag: &AtomicBool,
) -> Result<String, &'static str> {
    if format == "jwt-decode" {
        return jwt(raw, flag);
    }
    let document = compact(raw, flag)?;
    if !["auth-decode", "jws-decode", "jwe-inspect"].contains(&format) {
        return Err("Unsupported authentication conversion.");
    }
    if (format == "jwe-inspect") != (document.format == "JWE") && format != "auth-decode" {
        return Err("Input is not the requested JOSE format.");
    }
    if ["malformed", "limited"].contains(&document.status.as_str()) {
        return Err("Authentication structure is malformed or exceeds the parser limits.");
    }
    let result = document.scratch().pretty();
    canceled(flag)?;
    Ok(result)
}
