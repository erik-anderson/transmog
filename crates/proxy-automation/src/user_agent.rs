//! First-match User-Agent and existing client-hint replacements.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use transmog_core::{HeaderBlock, HeaderField};

/// User-Agent policy for one matching request. Missing headers are never added.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserAgentOverride {
    /// Replacement template; null preserves the original User-Agent.
    /// `%s` and `%full%` use the resolved full version, `%major%` its major.
    pub template: Option<String>,
    /// Explicit major or dotted version. Null reads the source browser's major.
    pub version_override: Option<String>,
    /// Replacements for existing Sec-CH-UA headers; empty preserves them.
    #[serde(default)]
    pub client_hints: BTreeMap<String, String>,
    /// Remove all existing Sec-CH-UA headers, including newer family members.
    #[serde(default)]
    pub strip_client_hints: bool,
}

impl UserAgentOverride {
    pub(crate) fn valid(&self) -> bool {
        let mut names = BTreeSet::new();
        self.template.as_ref().is_none_or(|value| {
            value.len() <= 4096 && HeaderField::try_new("User-Agent", value.as_bytes()).is_ok()
        }) && self
            .version_override
            .as_ref()
            .is_none_or(|version| valid_version(version))
            && self.client_hints.len() <= 32
            && (!self.strip_client_hints || self.client_hints.is_empty())
            && self.client_hints.iter().all(|(name, value)| {
                name.len() <= 128
                    && hint(name.as_bytes())
                    && names.insert(name.to_ascii_lowercase())
                    && value.len() <= 4096
                    && HeaderField::try_new(name.as_bytes(), value.as_bytes()).is_ok()
            })
            && (self.template.is_some() || !self.client_hints.is_empty() || self.strip_client_hints)
    }
    pub(crate) fn apply(&self, headers: &mut HeaderBlock) -> bool {
        let source = headers
            .values("user-agent")
            .filter_map(|value| std::str::from_utf8(value).ok())
            .find_map(source_major);
        let version = self
            .version_override
            .as_deref()
            .map(str::to_owned)
            .or_else(|| source.map(|major| format!("{major}.0.0.0")));
        let version = version.map(|version| {
            if version.contains('.') {
                version
            } else {
                format!("{version}.0.0.0")
            }
        });
        let major = version
            .as_deref()
            .and_then(|version| version.split('.').next());
        let hints = self
            .client_hints
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;
        let fields = headers
            .iter()
            .filter_map(|field| {
                if self.strip_client_hints && hint(field.name()) {
                    changed = true;
                    return None;
                }
                let value = if field.name_eq("user-agent") {
                    self.template.as_ref()
                } else {
                    std::str::from_utf8(field.name())
                        .ok()
                        .and_then(|name| hints.get(&name.to_ascii_lowercase()).copied())
                };
                let Some(value) =
                    value.and_then(|value| replace_version(value, major, version.as_deref()))
                else {
                    return Some(field.clone());
                };
                if field.value() == value.as_bytes() {
                    return Some(field.clone());
                }
                let Ok(replacement) =
                    HeaderField::try_new(field.name().to_vec(), value.into_bytes())
                else {
                    return Some(field.clone());
                };
                changed = true;
                Some(replacement)
            })
            .collect::<Vec<_>>();
        if changed {
            *headers = HeaderBlock::from_fields(fields);
        }
        changed
    }
}

fn hint(name: &[u8]) -> bool {
    name.len() >= 9
        && name[..9].eq_ignore_ascii_case(b"sec-ch-ua")
        && (name.len() == 9 || name[9] == b'-')
}
fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 32
        && version.split('.').count() <= 4
        && version.split('.').all(|part| {
            !part.is_empty() && part.len() <= 8 && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}
fn source_major(user_agent: &str) -> Option<&str> {
    for prefix in [
        "Edg/",
        "EdgA/",
        "EdgiOS/",
        "Edge/",
        "OPR/",
        "FxiOS/",
        "Firefox/",
        "CriOS/",
        "Chrome/",
        "Chromium/",
        "Version/",
    ] {
        for token in user_agent.split_ascii_whitespace() {
            if let Some(version) = token.trim_start_matches(['(', ';']).strip_prefix(prefix) {
                let end = version.bytes().take_while(u8::is_ascii_digit).count();
                if end > 0 && end <= 8 && version.as_bytes().get(end) == Some(&b'.') {
                    return Some(&version[..end]);
                }
            }
        }
    }
    None
}
fn replace_version(value: &str, major: Option<&str>, full: Option<&str>) -> Option<String> {
    if (value.contains("%s") || value.contains("%full%")) && full.is_none()
        || value.contains("%major%") && major.is_none()
    {
        return None;
    }
    Some(
        value
            .replace("%s", full.unwrap_or_default())
            .replace("%full%", full.unwrap_or_default())
            .replace("%major%", major.unwrap_or_default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn headers(fields: &[(&str, &str)]) -> HeaderBlock {
        HeaderBlock::from_fields(
            fields
                .iter()
                .map(|(name, value)| HeaderField::try_new(*name, *value).unwrap())
                .collect(),
        )
    }
    fn chrome() -> &'static str {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36"
    }
    #[test]
    fn edge_and_hints_use_source_major_and_preserve_duplicate_and_absent_headers() {
        let policy = UserAgentOverride {
            template: Some("Mozilla/5.0 Chrome/%s Safari/537.36 Edg/%s".into()),
            client_hints: BTreeMap::from([
                (
                    "Sec-CH-UA".into(),
                    "\"Chromium\";v=\"%major%\", \"Microsoft Edge\";v=\"%major%\"".into(),
                ),
                ("Sec-CH-UA-Platform".into(), "\"Windows\"".into()),
                ("Sec-CH-UA-Full-Version".into(), "\"%full%\"".into()),
            ]),
            ..UserAgentOverride::default()
        };
        let mut fields = headers(&[
            ("User-Agent", chrome()),
            ("Sec-CH-UA", "old"),
            ("sec-ch-ua", "duplicate"),
            ("Cookie", "keep"),
        ]);
        assert!(policy.apply(&mut fields));
        assert_eq!(
            fields.values("user-agent").next().unwrap(),
            b"Mozilla/5.0 Chrome/154.0.0.0 Safari/537.36 Edg/154.0.0.0"
        );
        assert_eq!(fields.values("sec-ch-ua").count(), 2);
        assert!(fields.values("sec-ch-ua-platform").next().is_none());
        assert_eq!(fields.values("cookie").next().unwrap(), b"keep");
        assert!(policy.valid());
    }
    #[test]
    fn explicit_version_unknown_source_and_hint_stripping_are_deterministic() {
        let mut policy = UserAgentOverride {
            template: Some("Chrome/%s Edg/%full%".into()),
            ..UserAgentOverride::default()
        };
        let mut fields = headers(&[
            ("User-Agent", "custom client"),
            ("Sec-CH-UA", "keep"),
            ("Sec-CH-UA-Form-Factors", "new hint"),
        ]);
        assert!(!policy.apply(&mut fields));
        assert_eq!(fields.values("sec-ch-ua").next().unwrap(), b"keep");
        policy.version_override = Some("155".into());
        assert!(policy.apply(&mut fields));
        assert_eq!(
            fields.values("user-agent").next().unwrap(),
            b"Chrome/155.0.0.0 Edg/155.0.0.0"
        );
        policy.version_override = Some("156.2.3.4".into());
        policy.strip_client_hints = true;
        assert!(policy.apply(&mut fields));
        assert!(fields.values("sec-ch-ua").next().is_none());
        assert!(fields.values("sec-ch-ua-form-factors").next().is_none());
        let mut absent = headers(&[("Accept", "*/*")]);
        assert!(!policy.apply(&mut absent));
        assert!(absent.values("user-agent").next().is_none());
        policy.version_override = Some("1\r\ninjected".into());
        assert!(!policy.valid());
    }
}
