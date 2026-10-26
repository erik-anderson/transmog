//! Bounded JSON syntax with ordered duplicate members and exact number spellings.
use crate::{Context, Finding, MAX_DEPTH, MAX_FIELDS};

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Object(Vec<(String, Node)>),
    Array(Vec<Node>),
    Scalar(String),
    Text(String),
    /// Original uninterpreted bytes; scratch JSON chooses an explicit lossless representation.
    Bytes(Vec<u8>),
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
            Self::Scalar(_) | Self::Text(_) | Self::Bytes(_) => false,
        }
    }

    /// Format syntax without losing duplicate members, order or number spellings.
    pub(crate) fn pretty(&self) -> String {
        fn write(node: &Node, depth: usize, output: &mut String) {
            let indent = "  ".repeat(depth);
            match node {
                Node::Object(members) => {
                    output.push('{');
                    for (index, (key, value)) in members.iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        output.push('\n');
                        output.push_str(&indent);
                        output.push_str("  ");
                        output.push_str(&serde_json::to_string(key).expect("JSON string"));
                        output.push_str(": ");
                        write(value, depth + 1, output);
                    }
                    if !members.is_empty() {
                        output.push('\n');
                        output.push_str(&indent);
                    }
                    output.push('}');
                }
                Node::Array(values) => {
                    output.push('[');
                    for (index, value) in values.iter().enumerate() {
                        if index > 0 {
                            output.push(',');
                        }
                        output.push('\n');
                        output.push_str(&indent);
                        output.push_str("  ");
                        write(value, depth + 1, output);
                    }
                    if !values.is_empty() {
                        output.push('\n');
                        output.push_str(&indent);
                    }
                    output.push(']');
                }
                Node::Scalar(value) => output.push_str(value),
                Node::Text(value) => {
                    output.push_str(&serde_json::to_string(value).expect("JSON string"));
                }
                Node::Bytes(bytes) => {
                    use base64::Engine as _;
                    write(
                        &Node::Object(vec![
                            ("encoding".into(), Node::Text("base64".into())),
                            (
                                "data".into(),
                                Node::Text(base64::engine::general_purpose::STANDARD.encode(bytes)),
                            ),
                        ]),
                        depth,
                        output,
                    );
                }
            }
        }
        let mut output = String::new();
        write(self, 0, &mut output);
        output
    }
}
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    nodes: usize,
    limited: bool,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| b" \t\r\n".contains(byte))
        {
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
            self.limited = true;
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
    parse_bounded(bytes).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    Malformed,
    Limited,
}

pub(crate) fn parse_bounded(bytes: &[u8]) -> Result<Node, ParseError> {
    let mut parser = Parser {
        bytes,
        at: 0,
        nodes: 0,
        limited: false,
    };
    let node = parser.node(0).ok_or(if parser.limited {
        ParseError::Limited
    } else {
        ParseError::Malformed
    })?;
    parser.space();
    if parser.at == bytes.len() {
        Ok(node)
    } else {
        Err(ParseError::Malformed)
    }
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
                    String::new()
                };
            context.field(finding, prefix, value, secret, &interpretation);
        }
        Node::Bytes(bytes) => {
            use base64::Engine as _;
            context.field(
                finding,
                prefix,
                base64::engine::general_purpose::STANDARD.encode(bytes),
                true,
                "Original bytes (Base64).",
            );
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
            if (value.floor() - at).abs() < 0.5 { return "At capture: ordering within the recorded time's one-second precision boundary is uncertain.".into(); }
            match role {
                "exp" if at >= value => "At capture: expiration condition reached.",
                "nbf" if at < value => "At capture: before not-before condition.",
                "iat" if at < value => "At capture: issued-at is in the future.",
                _ => "At capture: this time condition was not crossed.",
            }
            .into()
        },
    );
    format!("{utc} (UTC shown to whole seconds). {condition}")
}
