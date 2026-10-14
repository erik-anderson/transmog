//! Chromium 154's SAN-only hostname policy and ICANN wildcard boundary.
use std::{collections::HashSet, net::IpAddr, sync::OnceLock};

fn dns_name(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 || !name.is_ascii() {
        return None;
    }
    // The URL adapters supply canonical ASCII hostnames, including A-labels.
    if !name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) || !name
        .rsplit('.')
        .next()?
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
    {
        return None;
    }
    Some(name.to_ascii_lowercase())
}

fn registry_length(host: &str) -> usize {
    static RULES: OnceLock<HashSet<String>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        include_str!("../../../third_party/chromium-suffix-list/icann-rules.txt")
            .lines()
            .map(str::to_owned)
            .collect()
    });
    let labels: Vec<_> = host.split('.').collect();
    // INCLUDE_UNKNOWN_REGISTRIES: an unlisted TLD still prevents *.tld.
    let mut length = labels.last().map_or(0, |label| label.len());
    for index in 0..labels.len() {
        let suffix = labels[index..].join(".");
        if rules.contains(&format!("!{suffix}")) {
            return labels[index + 1..].join(".").len();
        }
        if rules.contains(&suffix)
            || index + 1 < labels.len()
                && rules.contains(&format!("*.{}", labels[index + 1..].join(".")))
        {
            length = length.max(suffix.len());
        }
    }
    length
}

fn matches_dns(reference: &str, presented: &str) -> bool {
    if presented.is_empty() || presented.contains('\0') || !presented.is_ascii() {
        return false;
    }
    let presented = presented
        .strip_suffix('.')
        .unwrap_or(presented)
        .to_ascii_lowercase();
    if presented == reference {
        return true;
    }
    let Some(domain) = presented.strip_prefix("*.") else {
        return false;
    };
    let Some((first, reference_domain)) = reference.split_once('.') else {
        return false;
    };
    !first.is_empty()
        && domain == reference_domain
        && registry_length(reference) != domain.len()
        && reference
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && byte != b'.')
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    pub dns: Vec<Vec<u8>>,
    pub ips: Vec<Vec<u8>>,
}

pub(crate) fn matches(names: &Names, host: &str) -> bool {
    let host = host.trim_matches(['[', ']']);
    if let Ok(ip) = host.parse::<IpAddr>() {
        let bytes = match ip {
            IpAddr::V4(ip) => ip.octets().to_vec(),
            IpAddr::V6(ip) => ip.octets().to_vec(),
        };
        return names.ips.contains(&bytes);
    }
    let Some(reference) = dns_name(host) else {
        return false;
    };
    names
        .dns
        .iter()
        .filter_map(|name| std::str::from_utf8(name).ok())
        .any(|name| matches_dns(&reference, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wildcard_boundary_matches_chromium_icann_policy() {
        for (reference, presented, expected) in [
            ("www.example.com", "*.example.com", true),
            ("a.b.example.com", "*.example.com", false),
            ("example.com", "*.com", false),
            ("example.co.uk", "*.co.uk", false),
            ("example.newtld", "*.newtld", false),
            ("x.appspot.com", "*.appspot.com", true),
            ("www.city.kawasaki.jp", "*.city.kawasaki.jp", true),
            ("y.x.kawasaki.jp", "*.x.kawasaki.jp", false),
            ("localhost", "*", false),
            ("www.example.com", "w*.example.com", false),
            ("www.example.com", "WWW.EXAMPLE.COM.", true),
            ("1.2.3", "*.2.3", false),
            ("www.example.com", "www.example.com\0.evil", false),
        ] {
            assert_eq!(
                matches_dns(reference, presented),
                expected,
                "{reference} / {presented}"
            );
        }
    }
    #[test]
    fn reference_names_follow_chromium_compliance_rules() {
        for name in [
            "_service.example.com",
            "-service.example.com",
            "localhost",
            "EXAMPLE.COM.",
        ] {
            assert!(dns_name(name).is_some(), "{name}");
        }
        for name in [
            "example._com",
            "example.-com",
            ".",
            "a..com",
            "example.com..",
            "a\0.com",
        ] {
            assert!(dns_name(name).is_none(), "{name}");
        }
    }
}
