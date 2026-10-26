//! Native catalog shared by conversion dispatch, presentation and evidence hints.
use serde::Serialize;

/// One supported explicit scratch operation. IDs are persistent draft contracts.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConversion {
    /// Stable conversion ID.
    pub id: &'static str,
    /// User-facing operation name.
    pub label: &'static str,
    /// Input and interpretation guidance beside the operation.
    pub hint: &'static str,
    /// Whether an otherwise ambiguous bare Base64 token is accepted explicitly.
    pub accepts_bare_base64: bool,
}

/// All supported scratch authentication operations, in presentation order.
pub const AUTH_CONVERSIONS: &[AuthConversion] = &[
    AuthConversion {
        id: "auth-decode",
        label: "Decode authentication (auto)",
        hint: "Paste an authentication header, scheme and value, or recognizable token. Output includes secrets. Nothing is verified or fetched.",
        accepts_bare_base64: false,
    },
    AuthConversion {
        id: "jwt-decode",
        label: "Decode JWT",
        hint: "Decodes the header and claims locally. Signatures are not verified.",
        accepts_bare_base64: false,
    },
    AuthConversion {
        id: "jws-decode",
        label: "Decode JWS",
        hint: "Decodes the protected header and payload, preserving binary payload bytes as Base64. Signatures are not verified.",
        accepts_bare_base64: false,
    },
    AuthConversion {
        id: "jwe-inspect",
        label: "Inspect JWE",
        hint: "Shows the protected header and encrypted component metadata. Claims remain encrypted; no decryption is performed.",
        accepts_bare_base64: false,
    },
    AuthConversion {
        id: "basic-decode",
        label: "Decode Basic",
        hint: "Accepts Basic credentials or bare Base64. Splits at the first colon and preserves bytes when the character set is unknown. Output includes the password.",
        accepts_bare_base64: true,
    },
    AuthConversion {
        id: "digest-parse",
        label: "Parse Digest",
        hint: "Parses ordered authentication parameters, including duplicates. Response hashes are not verified.",
        accepts_bare_base64: false,
    },
    AuthConversion {
        id: "ntlm-decode",
        label: "Decode NTLM",
        hint: "Decodes NTLM message metadata and available identities. Responses are not verified.",
        accepts_bare_base64: true,
    },
    AuthConversion {
        id: "negotiate-decode",
        label: "Decode Negotiate / SPNEGO",
        hint: "Decodes GSS/SPNEGO and confirmed NTLM or Kerberos children. Encrypted contents remain unavailable.",
        accepts_bare_base64: true,
    },
];

/// Operation hint for known parser formats; uncertain wrappers use explicit Auto.
pub fn conversion_for_format(format: &str) -> Option<&'static str> {
    match format {
        "JWT / JWS" => Some("jwt-decode"),
        "JWS" => Some("jws-decode"),
        "JWE" => Some("jwe-inspect"),
        "Basic" => Some("basic-decode"),
        "Digest" => Some("digest-parse"),
        "NTLM" => Some("ntlm-decode"),
        "Negotiate" | "GSS-API / SPNEGO" | "SPNEGO" | "Kerberos V5" => Some("negotiate-decode"),
        "Bearer" | "Unknown HTTP auth scheme" => Some("auth-decode"),
        _ => None,
    }
}
