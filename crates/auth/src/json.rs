//! Bounded JSON syntax with ordered duplicate members and exact number spellings.
use crate::{Context, Finding, MAX_DEPTH, MAX_FIELDS};

#[derive(Debug)]
pub(crate) enum Node {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    Scalar(String),
    Text(String),
}
impl Node {
    pub(crate) fn members(&self) -> Option<&[(String, Self)]> {
        if let Self::Object(values) = self {
            Some(values)
        } else {
            None
        }
    }
    pub(crate) fn text(&self) -> Option<&str> {
        if let Self::Scalar(value) | Self::Text(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub(crate) fn string(&self) -> Option<&str> {
        if let Self::Text(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub(crate) fn get(&self, name: &str) -> Option<&Self> {
        let values = self.members()?;
        let mut found = values.iter().filter(|(key, _)| key == name);
        let value = found.next()?;
        if found.next().is_some() {
            return None;
        }
        Some(&value.1)
    }
    pub(crate) fn duplicate(&self) -> bool {
        match self {
            Self::Object(values) => values.iter().enumerate().any(|(i, (key, value))| {
                values[..i].iter().any(|(k, _)| k == key) || value.duplicate()
            }),
            Self::Array(values) => values.iter().any(Self::duplicate),
            Self::Scalar(_) | Self::Text(_) => false,
        }
    }
}
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn string(&mut self) -> Option<String> {
        let start = self.at;
        if self.bytes.get(self.at) != Some(&b'"') {
            return None;
        }
        self.at += 1;
        while let Some(b) = self.bytes.get(self.at) {
            self.at += 1;
            if *b == b'\\' {
                self.at += 1;
            }
            if *b == b'"' {
                return serde_json::from_slice(self.bytes.get(start..self.at)?).ok();
            }
        }
        None
    }
    fn node(&mut self, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH || self.nodes >= MAX_FIELDS {
            return None;
        }
        self.nodes += 1;
        self.space();
        match *self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                self.space();
                let mut values = vec![];
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Some(Node::Object(values));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.space();
                    if self.bytes.get(self.at) != Some(&b':') {
                        return None;
                    }
                    self.at += 1;
                    values.push((key, self.node(depth + 1)?));
                    self.space();
                    match self.bytes.get(self.at)? {
                        b'}' => {
                            self.at += 1;
                            break;
                        }
                        b',' => self.at += 1,
                        _ => return None,
                    }
                }
                Some(Node::Object(values))
            }
            b'[' => {
                self.at += 1;
                self.space();
                let mut values = vec![];
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Some(Node::Array(values));
                }
                loop {
                    values.push(self.node(depth + 1)?);
                    self.space();
                    match self.bytes.get(self.at)? {
                        b']' => {
                            self.at += 1;
                            break;
                        }
                        b',' => self.at += 1,
                        _ => return None,
                    }
                }
                Some(Node::Array(values))
            }
            b'"' => self.string().map(Node::Text),
            _ => {
                let start = self.at;
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
                {
                    self.at += 1;
                }
                let raw = self.bytes.get(start..self.at)?;
                if !raw
                    .first()
                    .is_some_and(|b| b.is_ascii_digit() || b"-tfn".contains(b))
                {
                    return None;
                }
                serde_json::from_slice::<serde::de::IgnoredAny>(raw).ok()?;
                Some(Node::Scalar(std::str::from_utf8(raw).ok()?.into()))
            }
        }
    }
}
pub(crate) fn parse(bytes: &[u8]) -> Option<Node> {
    let mut parser = Parser {
        bytes,
        at: 0,
        nodes: 0,
    };
    let node = parser.node(0)?;
    parser.space();
    (parser.at == bytes.len()).then_some(node)
}
pub(crate) fn fields(
    context: &mut Context,
    finding: &mut Finding,
    node: &Node,
    prefix: &str,
    secret: bool,
) {
    if !context.step() {
        context.limited();
        return;
    }
    match node {
        Node::Object(values) => {
            for (key, value) in values {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                let sensitive = secret
                    || [
                        "nonce",
                        "jti",
                        "access_token",
                        "refresh_token",
                        "id_token",
                        "client_secret",
                        "password",
                        "code",
                        "state",
                        "code_verifier",
                    ]
                    .contains(&key.as_str());
                fields(context, finding, value, &path, sensitive);
            }
        }
        Node::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                fields(context, finding, value, &format!("{prefix}[{i}]"), secret);
            }
        }
        Node::Scalar(value) | Node::Text(value) => {
            let interpretation =
                if matches!(node, Node::Scalar(_)) && ["iat", "nbf", "exp"].contains(&prefix) {
                    numeric_date(value, prefix, context.captured_at)
                } else {
                    "Captured, unverified value.".into()
                };
            context.field(finding, prefix, value, secret, &interpretation);
        }
    }
}
pub(crate) fn numeric_date(raw: &str, role: &str, captured: Option<i64>) -> String {
    // The original NumericDate spelling remains the field value, including fractional or very large numbers.
    let Ok(value) = raw.parse::<f64>() else {
        return "NumericDate is not a supported number; raw value preserved.".into();
    };
    if !value.is_finite() || value.abs() > 253_402_300_799.0 {
        return "NumericDate is outside the supported UTC display range; raw value preserved."
            .into();
    }
    #[allow(clippy::cast_possible_truncation)]
    let utc = time::OffsetDateTime::from_unix_timestamp(value.floor() as i64)
        .ok()
        .and_then(|v| {
            v.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "UTC unavailable".into());
    let condition: String = captured.map_or_else(
        || "Capture time unavailable; no time condition comparison.".into(),
        |at| {
            #[allow(clippy::cast_precision_loss)]
            let at = at as f64;
            match role {
                "exp" if at >= value => "At capture: expiration condition reached.",
                "nbf" if at < value => "At capture: before not-before condition.",
                "iat" if at < value => "At capture: issued-at is in the future.",
                _ => "At capture: this time condition was not crossed.",
            }
            .into()
        },
    );
    format!("{utc}. {condition} Unverified; this does not establish token validity.")
}
