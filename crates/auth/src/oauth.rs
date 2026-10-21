//! Contextual protocol field sets; a generic JSON code/state/error alone is not OAuth.
use crate::{Context, Finding, extract::Pair, jose};

pub(crate) fn role(items: &[Pair]) -> Option<&'static str> {
    let has = |name: &str| items.iter().any(|p| p.name == name);
    if has("client_id") && has("response_type") {
        return Some("authorization request");
    }
    if has("grant_type")
        && [
            "client_id",
            "code",
            "refresh_token",
            "client_assertion",
            "username",
        ]
        .iter()
        .any(|key| has(key))
    {
        return Some("token request");
    }
    if has("access_token") && has("token_type") || has("id_token") {
        return Some("token response / OIDC evidence");
    }
    let callback_context = items.first().is_some_and(|p| p.source.kind != "body")
        || has("client_id")
        || has("redirect_uri");
    if callback_context && has("state") && (has("code") || has("error") || has("access_token")) {
        return Some("authorization callback");
    }
    None
}
pub(crate) fn explanation(name: &str) -> &'static str {
    match name {
        "client_id" => {
            "Observed client identifier; endpoint paths alone do not identify a provider."
        }
        "redirect_uri" => "Observed redirect URI; inert captured data.",
        "scope" => "Requested or returned space-separated scope values.",
        "response_type" => {
            "Observed authorization response type, including code, implicit or hybrid shapes."
        }
        "response_mode" => "Observed response delivery mode.",
        "grant_type" => {
            "Observed grant, including legacy or extension grants; no grant is executed."
        }
        "code" => "Opaque authorization code; never redeemed or decoded by guessing.",
        "state" => "Sensitive correlation value; does not alone establish a relationship.",
        "nonce" => "Sensitive OIDC correlation value; unverified.",
        "code_challenge" => "PKCE challenge; cross-request consistency is not checked.",
        "code_challenge_method" => "Observed PKCE transformation method.",
        "code_verifier" => "Sensitive PKCE verifier; never sent or validated.",
        "id_token" => "ID-token role follows observed OIDC context; signature not checked.",
        "access_token" => {
            "Access token may be opaque; Bearer alone does not establish OAuth or JWT."
        }
        "refresh_token" => "Opaque refresh credential; never exchanged.",
        "expires_in" => {
            "Relative lifetime only; no absolute expiry inferred without a suitable observed response time."
        }
        "client_secret" => "Sensitive client authentication material.",
        "client_assertion" | "client_assertion_type" => {
            "Client assertion in its declared authentication context; not verified."
        }
        "error_uri" => "Captured error URI is inert; no request is made.",
        _ => "Observed protocol field; no authentication or trust decision is made.",
    }
}
pub(crate) fn public(name: &str) -> bool {
    [
        "client_id",
        "redirect_uri",
        "scope",
        "response_type",
        "response_mode",
        "grant_type",
        "code_challenge",
        "code_challenge_method",
        "token_type",
        "expires_in",
        "client_assertion_type",
        "error",
        "error_description",
        "error_uri",
    ]
    .contains(&name)
}
pub(crate) fn decode(context: &mut Context, items: &[Pair]) -> Option<Finding> {
    let role = role(items)?;
    let mut finding = Finding::new(
        "OAuth / OpenID Connect",
        role,
        items.first()?.source.clone(),
    );
    finding.source.offset = None;
    finding.source.length = None;
    for (index, pair) in items.iter().enumerate() {
        if !context.step() {
            context.limited();
            break;
        }
        if items[..index].iter().any(|p| p.name == pair.name) {
            finding.issue(
                "incomplete",
                "Duplicate protocol parameters are retained and ambiguous.",
            );
        }
        context.field(
            &mut finding,
            &pair.name,
            &pair.value,
            !public(&pair.name),
            explanation(&pair.name),
        );
        // Original encodings and absent delimiters remain available without leaking through default serialization.
        context.field(
            &mut finding,
            &format!(
                "{} · original field {}",
                pair.name,
                pair.source.occurrence + 1
            ),
            &pair.original,
            true,
            if pair.delimiter {
                "Original encoded representation; '=' was present."
            } else {
                "Original encoded representation; no '=' delimiter."
            },
        );
        if ["access_token", "id_token", "client_assertion"].contains(&pair.name.as_str())
            && let Some(mut inner) = jose::decode(context, &pair.value, pair.source.clone())
        {
            inner.role = explanation(&pair.name).into();
            finding.children.push(inner);
        }
    }
    Some(finding)
}
