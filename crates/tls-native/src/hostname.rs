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
    registry_length_with_unknown(host, true)
}

fn registry_length_with_unknown(host: &str, include_unknown: bool) -> usize {
    static RULES: OnceLock<HashSet<String>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        include_str!("../../../third_party/chromium-suffix-list/icann-rules.txt")
            .lines()
            .map(str::to_owned)
            .collect()
    });
    let labels: Vec<_> = host.split('.').collect();
    // INCLUDE_UNKNOWN_REGISTRIES: an unlisted TLD still prevents *.tld.
    let mut length = if include_unknown {
        labels.last().map_or(0, |label| label.len())
    } else {
        0
    };
    for index in 0..labels.len() {
        let suffix = labels[index..].join(".");
        if rules.contains(&format!("!{suffix}")) {
            return labels[index + 1..].join(".").len();
        }
        if rules.contains(&suffix)
            || !include_unknown && rules.contains(&format!("*.{suffix}"))
            || index + 1 < labels.len()
                && rules.contains(&format!("*.{}", labels[index + 1..].join(".")))
        {
            length = length.max(suffix.len());
        }
    }
    length
}

/// Chromium's public-certificate non-unique-name policy. Invalid names are
/// handled by hostname matching instead of being mislabeled as non-unique.
pub(crate) fn non_unique(host: &str) -> bool {
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(ip) => !public_ipv4(u32::from(ip)),
            IpAddr::V6(ip) => {
                if let Some(ip) = ip.to_ipv4_mapped() {
                    !public_ipv4(u32::from(ip))
                } else {
                    ip.segments()[0] & 0xe000 != 0x2000 && ip.octets()[0] != 0xff
                }
            }
        };
    }
    dns_name(host).is_some_and(|name| registry_length_with_unknown(&name, false) == 0)
}

fn public_ipv4(ip: u32) -> bool {
    // net/base/ip_address.cc's exact Chromium 154 reservations.
    const RESERVED: [(u32, u32); 14] = [
        (0x0000_0000, 8),
        (0x0a00_0000, 8),
        (0x6440_0000, 10),
        (0x7f00_0000, 8),
        (0xa9fe_0000, 16),
        (0xac10_0000, 12),
        (0xc000_0000, 24),
        (0xc000_0200, 24),
        (0xc058_6300, 24),
        (0xc0a8_0000, 16),
        (0xc612_0000, 15),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
        (0xe000_0000, 3),
    ];
    !RESERVED
        .iter()
        .any(|(prefix, bits)| ip & (u32::MAX << (32 - bits)) == *prefix)
}

pub(crate) fn restricted_key_names(hashes: &[[u8; 32]], names: &Names) -> bool {
    // CertVerifyProc::HasNameConstraintsViolation; the test-only example key
    // is not a production restriction. Only public DNS subdomains are limited.
    const ANSSI: [u8; 32] = [
        0x86, 0xc1, 0x3a, 0x34, 0x08, 0xdd, 0x1a, 0xa7, 0x7e, 0xe8, 0xb6, 0x94, 0x7c, 0x03, 0x95,
        0x87, 0x72, 0xf5, 0x31, 0x24, 0x8c, 0x16, 0x27, 0xbe, 0xfb, 0x2c, 0x4f, 0x4b, 0x04, 0xd0,
        0x44, 0x96,
    ];
    const DOMAINS: [&str; 13] = [
        "fr", "gp", "gf", "mq", "re", "yt", "pm", "bl", "mf", "wf", "pf", "nc", "tf",
    ];
    hashes.contains(&ANSSI)
        && names.dns.iter().any(|name| {
            let Ok(name) = std::str::from_utf8(name) else {
                return false;
            };
            let name = name.trim_start_matches("*.");
            // Unlike the expected URL name, SAN values can have noncompliant
            // labels. Their public registry still participates in this limit.
            if name.is_empty() || name.contains(['\0', '/', '\\', ':', '#', '?', '@', '[', ']']) {
                return false;
            }
            let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
            let registry = registry_length_with_unknown(&name, false);
            if registry == 0 || name.len() <= registry || name.parse::<IpAddr>().is_ok() {
                return false;
            }
            !DOMAINS
                .iter()
                .any(|suffix| name.ends_with(&format!(".{suffix}")))
        })
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
    fn public_name_policy_matches_chromium_registry_and_ip_boundaries() {
        for name in [
            "printer",
            "printer.internal",
            "localhost",
            "127.0.0.1",
            "10.1.2.3",
            "100.64.0.1",
            "192.0.2.1",
            "::1",
            "::ffff:192.168.1.1",
            "fc00::1",
        ] {
            assert!(non_unique(name), "{name}");
        }
        for name in [
            "example.com",
            "com",
            "co.uk",
            "ck",
            "abc.ck",
            "a.blogspot.com",
            "example.com.",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "2001:db8::1",
            "ff02::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!non_unique(name), "{name}");
        }
        assert!(!non_unique("invalid\0name"));
    }

    #[test]
    fn fixed_key_limits_skip_private_names_and_require_permitted_public_dns() {
        let key = [
            0x86, 0xc1, 0x3a, 0x34, 0x08, 0xdd, 0x1a, 0xa7, 0x7e, 0xe8, 0xb6, 0x94, 0x7c, 0x03,
            0x95, 0x87, 0x72, 0xf5, 0x31, 0x24, 0x8c, 0x16, 0x27, 0xbe, 0xfb, 0x2c, 0x4f, 0x4b,
            0x04, 0xd0, 0x44, 0x96,
        ];
        let names = |dns: &[&str]| Names {
            dns: dns.iter().map(|s| s.as_bytes().to_vec()).collect(),
            ips: vec![],
        };
        assert!(!restricted_key_names(
            &[key],
            &names(&["gouv.fr", "*.gouv.fr", "printer.internal", "fr"])
        ));
        assert!(restricted_key_names(
            &[key],
            &names(&["gouv.fr", "other.com"])
        ));
        assert!(restricted_key_names(
            &[key],
            &names(&["good.fr", "invalid*.com"])
        ));
        assert!(!restricted_key_names(&[], &names(&["other.com"])));
    }
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
