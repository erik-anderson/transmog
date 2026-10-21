//! Namespace-aware SAML metadata and bounded Redirect decoding. XML is never rendered or resolved.
use crate::{Context, Finding, MAX_DEPTH, Source, extract::Pair};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use quick_xml::{NsReader, events::Event, name::ResolveResult};
use std::io::Read as _;

pub(crate) fn parameter(context: &mut Context, pair: &Pair, items: &[Pair]) -> Finding {
    let mut source = pair.source.clone();
    source.transforms.push("Base64".into());
    let mut finding = Finding::new("SAML 2.0", &pair.name, source.clone());
    finding.verification = "Not checked".into();
    if pair.value.len() > crate::MAX_CANDIDATE || pair.original.len() > crate::MAX_CANDIDATE {
        finding.issue(
            "limited",
            "SAML binding value exceeded the encoded candidate budget; decoding was not attempted.",
        );
        return finding;
    }
    context.field(&mut finding, "Original encoded field", &pair.original, true, "Exact binding representation, including original URL/form escaping; signature not checked.");
    if pair.name == "SAMLart" {
        finding.status = "opaque".into();
        context.field(
            &mut finding,
            "Artifact",
            &pair.value,
            true,
            "Opaque SAML artifact; never resolved over the network.",
        );
        if let Ok(bytes) = STANDARD.decode(&pair.value) {
            context.field(
                &mut finding,
                "Artifact bytes",
                bytes.len().to_string(),
                false,
                "Decoded artifact length.",
            );
            if bytes.len() == 44 && bytes.starts_with(&[0, 4]) {
                context.field(&mut finding, "Artifact type", "0x0004", false, "SAML 2.0 artifact with endpoint index, source ID and message handle; not redeemed.");
            }
        }
        return finding;
    }
    let Ok(mut bytes) = STANDARD.decode(&pair.value) else {
        finding.issue("malformed", "SAML binding value has invalid Base64.");
        return finding;
    };
    if !context.charge(bytes.len()) {
        finding.status = "limited".into();
        return finding;
    }
    if ["query", "redirect query", "redirect fragment"].contains(&pair.source.kind.as_str()) {
        let mut inflater = flate2::read::DeflateDecoder::new(bytes.as_slice());
        let mut decoded = vec![];
        let outcome = (&mut inflater)
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut decoded);
        if decoded.len() > 4 * 1024 * 1024 || !context.charge(decoded.len()) {
            finding.issue(
                "limited",
                "SAML raw DEFLATE output exceeded the decoded budget.",
            );
            return finding;
        }
        if outcome.is_err() || inflater.total_in() != bytes.len() as u64 {
            finding.issue(
                "malformed",
                "Invalid or trailing raw DEFLATE data in SAML Redirect binding.",
            );
            return finding;
        }
        bytes = decoded;
        source.transforms.push("Raw DEFLATE".into());
    }
    if let Some(decoded) = xml(context, &bytes, source) {
        finding.children.push(decoded);
    } else {
        finding.issue(
            "malformed",
            "Binding did not contain a supported namespace-aware SAML message.",
        );
    }
    for metadata in items
        .iter()
        .filter(|p| ["RelayState", "SigAlg", "Signature"].contains(&p.name.as_str()))
    {
        context.field(
            &mut finding,
            &metadata.name,
            &metadata.value,
            metadata.name != "SigAlg",
            "Observed binding metadata; Redirect signatures are not checked.",
        );
    }
    finding
}
pub(crate) fn xml(context: &mut Context, bytes: &[u8], mut source: Source) -> Option<Finding> {
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().check_end_names = true;
    let mut finding = Finding::new("SAML 2.0 XML", "message / assertion", source.clone());
    finding.verification = "Not checked".into();
    let mut stack: Vec<(String, bool)> = vec![];
    let mut recognized = false;
    let mut roots = 0;
    let mut encrypted = false;
    let mut failure = None;
    loop {
        if !context.step() {
            failure = Some(("limited", "XML work budget exhausted."));
            break;
        }
        let event = reader.read_resolved_event();
        let empty = matches!(&event, Ok((_, Event::Empty(_))));
        match event {
            Ok((namespace, Event::Start(element) | Event::Empty(element))) => {
                if stack.is_empty() {
                    roots += 1;
                }
                if roots > 1 || stack.len() >= MAX_DEPTH {
                    failure = Some(("limited", "XML depth or root-element limit exceeded."));
                    break;
                }
                let saml = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == "urn:oasis:names:tc:SAML:2.0:protocol" || ns.as_ref() == "urn:oasis:names:tc:SAML:2.0:assertion");
                let signature = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == "http://www.w3.org/2000/09/xmldsig#");
                let encryption = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == "http://www.w3.org/2001/04/xmlenc#");
                let name = element.local_name().as_ref().to_owned();
                if saml {
                    recognized = true;
                    context.field(
                        &mut finding,
                        "Element",
                        &name,
                        false,
                        "Element recognized by its SAML 2.0 namespace.",
                    );
                }
                if signature && name == "Signature" {
                    context.field(
                        &mut finding,
                        "Signature",
                        "Present · Not checked",
                        false,
                        "Original XML remains available; reformatting is not signature validation.",
                    );
                }
                if encryption || saml && name.starts_with("Encrypted") {
                    encrypted = true;
                }
                for (index, attribute) in element.attributes().enumerate() {
                    if index >= 128 {
                        failure = Some((
                            "limited",
                            "XML attribute count exceeded the per-element limit.",
                        ));
                        break;
                    }
                    let Ok(attribute) = attribute else {
                        failure = Some(("malformed", "Malformed or duplicate XML attributes."));
                        break;
                    };
                    if !saml && !signature && !encryption {
                        continue;
                    }
                    let Ok(value) = attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    else {
                        failure =
                            Some(("malformed", "Unsupported XML attribute entity or encoding."));
                        break;
                    };
                    let key = attribute.key.as_ref();
                    if key.starts_with("xmlns") {
                        continue;
                    }
                    let note = if ["NotBefore", "NotOnOrAfter"].contains(&key) {
                        time_note(&value, key, context.captured_at)
                    } else {
                        "Unverified captured XML attribute.".into()
                    };
                    let sensitive = ["SessionIndex", "InResponseTo", "ID"].contains(&key)
                        || signature && !["Algorithm", "URI"].contains(&key);
                    context.field(
                        &mut finding,
                        &format!("{name} @{key}"),
                        value,
                        sensitive,
                        &note,
                    );
                }
                if !empty {
                    stack.push((name, saml || signature || encryption));
                }
            }
            Ok((_, Event::End(_))) => {
                if stack.pop().is_none() {
                    failure = Some(("malformed", "Unbalanced XML elements."));
                    break;
                }
            }
            Ok((_, Event::Text(text))) => {
                if let Some((name, true)) = stack.last() {
                    let raw = text.xml_content(quick_xml::XmlVersion::Implicit1_0);
                    let Ok(value) = quick_xml::escape::unescape(&raw) else {
                        failure = Some((
                            "malformed",
                            "Unsupported XML entity; external entities are disabled.",
                        ));
                        break;
                    };
                    if !value.trim().is_empty() {
                        context.field(&mut finding, name, value.trim(), !["Issuer", "Audience", "AuthnContextClassRef"].contains(&name.as_str()), "Unverified captured XML text. Attributes and subject values may contain personal data.");
                    }
                }
            }
            Ok((_, Event::CData(text))) => {
                if let Some((name, true)) = stack.last() {
                    context.field(
                        &mut finding,
                        name,
                        text.as_ref(),
                        true,
                        "Captured CDATA; inert text.",
                    );
                }
            }
            Ok((_, Event::GeneralRef(reference))) => {
                let value = reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .map(|c| c.to_string())
                    .or_else(|| {
                        quick_xml::escape::resolve_xml_entity(reference.as_ref()).map(str::to_owned)
                    });
                if let Some(value) = value {
                    if let Some((name, true)) = stack.last() {
                        context.field(&mut finding, name, value, true, "Decoded XML entity in the enclosing element; fragments preserve their original ordering.");
                    }
                } else {
                    failure = Some((
                        "unsupported",
                        "Unknown XML entity; external entity resolution is disabled.",
                    ));
                    break;
                }
            }
            Ok((_, Event::DocType(_))) => {
                failure = Some((
                    "unsupported",
                    "DTDs and external entity resolution are disabled.",
                ));
                break;
            }
            Ok((_, Event::Eof)) => {
                if !stack.is_empty() {
                    failure = Some(("incomplete", "XML capture ended inside an element."));
                }
                break;
            }
            Err(_) => {
                failure = Some((
                    "malformed",
                    "Malformed XML; no external resources were resolved.",
                ));
                break;
            }
            _ => {}
        }
    }
    if !recognized {
        if let Some((status, detail)) = failure {
            context.coverage(&source.kind, status, detail);
        }
        return None;
    }
    source.transforms.push("Namespace-aware XML parsing".into());
    finding.source = source;
    context.field(&mut finding, "Decoded XML", String::from_utf8_lossy(bytes), true, "Exact decoded XML text; gated because it includes assertions, signatures and personal data. No precise mapping to compressed bytes is claimed.");
    if encrypted {
        finding.status = "encrypted".into();
        finding.diagnostics.push("Encrypted assertions/IDs or cipher data are opaque; only exposed outer metadata is readable.".into());
    }
    if let Some((status, note)) = failure {
        finding.issue(status, note);
    }
    Some(finding)
}
fn time_note(raw: &str, name: &str, capture: Option<i64>) -> String {
    let Ok(time) = time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
    else {
        return "Unverified SAML time; UTC interpretation unavailable.".into();
    };
    capture.map_or_else(
        || "Capture time unavailable; no condition comparison. Unverified.".into(),
        |capture| {
            if capture == time.unix_timestamp() { return "At capture: ordering within the recorded time's one-second precision boundary is uncertain. Unverified.".into(); }
            let crossed = if name == "NotBefore" {
                capture < time.unix_timestamp()
            } else {
                capture >= time.unix_timestamp()
            };
            if crossed {
                "At capture: this condition was outside its declared interval. Unverified."
            } else {
                "At capture: this condition was inside its declared interval. Unverified."
            }
            .into()
        },
    )
}
