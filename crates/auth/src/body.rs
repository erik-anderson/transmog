//! Selected-message text extraction. HTML is tokenized inertly, never executed or rendered.
use crate::{
    Body, Context, MAX_DEPTH, Source,
    extract::{self, Pair},
    json, saml,
};

pub(crate) fn scan(context: &mut Context, body: Body) {
    let mime = body
        .media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let Some(text) = text(&body.bytes, &body.media_type) else {
        context.coverage(
            "body",
            "unsupported",
            "Unsupported, ambiguous or malformed body character encoding.",
        );
        return;
    };
    let mut source = Source {
        kind: "body".into(),
        label: "Decoded body".into(),
        transforms: body.transforms,
        ..Source::default()
    };
    source
        .transforms
        .push(character_transform(&body.bytes).into());
    source.label = "Character-decoded body".into();
    if mime == "application/x-www-form-urlencoded" {
        let items = extract::pairs(context, &text, &source, true);
        extract::parameters(context, &items);
    } else if mime == "application/json" || mime.ends_with("+json") {
        let Some(node) = json::parse(text.as_bytes()) else {
            context.coverage(
                "body",
                "incomplete",
                "JSON is malformed or exceeds depth/node limits.",
            );
            return;
        };
        json_groups(context, &node, "$", &source, 0);
    } else if mime.ends_with("/xml") || mime.ends_with("+xml") {
        if let Some(finding) = saml::xml(context, text.as_bytes(), source) {
            context.push(finding);
        }
    } else if mime == "text/html" {
        let items = html(context, &text, &source);
        extract::parameters(context, &items);
    } else {
        context.coverage("body", "unsupported", "Only form, JSON, XML/SOAP and inert hidden HTML controls are scanned; no file parts or arbitrary binary scan.");
        return;
    }
    if context
        .result
        .coverage
        .iter()
        .any(|c| c.source == "body" && c.status != "complete")
    {
        return;
    }
    context.coverage(
        "body",
        "complete",
        "Bounded selected-message text scan; unknown content remains uninterpreted.",
    );
}
pub(crate) fn character_transform(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xff, 0xfe]) {
        "Character decoding: UTF-16LE (BOM) to UTF-8"
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        "Character decoding: UTF-16BE (BOM) to UTF-8"
    } else {
        "Character decoding: UTF-8 / ASCII"
    }
}
pub(crate) fn text(bytes: &[u8], media: &str) -> Option<String> {
    let charset = media.split(';').skip(1).find_map(|p| {
        p.trim()
            .split_once('=')
            .filter(|(key, _)| key.eq_ignore_ascii_case("charset"))
            .map(|(_, value)| value.trim_matches('"').to_ascii_lowercase())
    });
    let little = bytes.starts_with(&[0xff, 0xfe]);
    let big = bytes.starts_with(&[0xfe, 0xff]);
    if little || big {
        if little && charset.as_deref() == Some("utf-16be")
            || big && charset.as_deref() == Some("utf-16le")
        {
            return None;
        }
        if charset
            .as_deref()
            .is_some_and(|v| !["utf-16", "utf-16le", "utf-16be"].contains(&v))
            || !(bytes.len() - 2).is_multiple_of(2)
        {
            return None;
        }
        let words: Vec<_> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                if little {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        return String::from_utf16(&words).ok();
    }
    if charset
        .as_deref()
        .is_some_and(|v| !["utf-8", "utf8", "us-ascii"].contains(&v))
    {
        return None;
    }
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if charset.as_deref() == Some("us-ascii") && !bytes.is_ascii() {
        return None;
    }
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}
fn json_groups(
    context: &mut Context,
    node: &json::Node,
    path: &str,
    source: &Source,
    depth: usize,
) {
    if depth > MAX_DEPTH || !context.step() {
        context.limited();
        return;
    }
    match node {
        json::Node::Object(values) => {
            let items: Vec<_> = values
                .iter()
                .enumerate()
                .filter_map(|(index, (name, value))| {
                    let value = value.text()?;
                    let mut source = source.clone();
                    source.label = format!("JSON {path}");
                    source.occurrence = index;
                    source.transforms.push("JSON scalar extraction".into());
                    Some(Pair {
                        name: name.clone(),
                        value: value.into(),
                        original: value.into(),
                        delimiter: true,
                        source,
                    })
                })
                .collect();
            extract::parameters(context, &items);
            for (key, value) in values {
                if ["access_token", "id_token", "client_assertion"].contains(&key.as_str()) {
                    continue;
                }
                json_groups(context, value, &format!("{path}.{key}"), source, depth + 1);
            }
        }
        json::Node::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                json_groups(
                    context,
                    value,
                    &format!("{path}[{index}]"),
                    source,
                    depth + 1,
                );
            }
        }
        json::Node::Text(value) => {
            if value.len() > crate::MAX_CANDIDATE {
                context.coverage(
                    "body",
                    "limited",
                    "A JSON scalar exceeded the auth candidate budget and was not interpreted.",
                );
                return;
            }
            let mut source = source.clone();
            source.label = format!("JSON {}", crate::clip(path, 512));
            source.transforms.push("JSON scalar extraction".into());
            if let Some(mut finding) = crate::jose::decode(context, value, source) {
                finding.role = "Compact JOSE structure in JSON scalar".into();
                context.push(finding);
            }
        }
        json::Node::Scalar(_) => {}
    }
}
fn entities(value: &str) -> String {
    let mut output = String::new();
    let mut at = 0;
    while at < value.len() {
        let tail = &value[at..];
        if tail.starts_with('&') {
            let mut matched = None;
            if let Some(number) = tail.strip_prefix("&#") {
                let (radix, number) = if let Some(hex) = number
                    .strip_prefix('x')
                    .or_else(|| number.strip_prefix('X'))
                {
                    (16, hex)
                } else {
                    (10, number)
                };
                let digits = number
                    .bytes()
                    .take_while(|b| {
                        if radix == 16 {
                            b.is_ascii_hexdigit()
                        } else {
                            b.is_ascii_digit()
                        }
                    })
                    .count();
                if digits > 0
                    && digits <= 8
                    && let Ok(code) = u32::from_str_radix(&number[..digits], radix)
                {
                    let prefix = tail.len() - number.len();
                    let length =
                        prefix + digits + usize::from(number.as_bytes().get(digits) == Some(&b';'));
                    let code = match code {
                        0 => 0xfffd,
                        0x80 => 0x20ac,
                        0x82 => 0x201a,
                        0x83 => 0x0192,
                        0x84 => 0x201e,
                        0x85 => 0x2026,
                        0x86 => 0x2020,
                        0x87 => 0x2021,
                        0x88 => 0x02c6,
                        0x89 => 0x2030,
                        0x8a => 0x0160,
                        0x8b => 0x2039,
                        0x8c => 0x0152,
                        0x8e => 0x017d,
                        0x91 => 0x2018,
                        0x92 => 0x2019,
                        0x93 => 0x201c,
                        0x94 => 0x201d,
                        0x95 => 0x2022,
                        0x96 => 0x2013,
                        0x97 => 0x2014,
                        0x98 => 0x02dc,
                        0x99 => 0x2122,
                        0x9a => 0x0161,
                        0x9b => 0x203a,
                        0x9c => 0x0153,
                        0x9e => 0x017e,
                        0x9f => 0x0178,
                        _ => code,
                    };
                    matched = Some((
                        length,
                        char::from_u32(code).unwrap_or('\u{fffd}').to_string(),
                    ));
                }
            } else {
                for length in (1..tail.len().min(34)).rev() {
                    if !tail.is_char_boundary(1 + length) {
                        continue;
                    }
                    let name = &tail[1..=length];
                    if !name.ends_with(';')
                        && tail
                            .as_bytes()
                            .get(1 + length)
                            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'=')
                    {
                        continue;
                    }
                    if let Some(&(a, b)) = web_atoms::NAMED_ENTITIES.get(name) {
                        let mut text = char::from_u32(a).unwrap_or('\u{fffd}').to_string();
                        if b != 0 {
                            text.push(char::from_u32(b).unwrap_or('\u{fffd}'));
                        }
                        matched = Some((length + 1, text));
                        break;
                    }
                }
            }
            if let Some((length, text)) = matched {
                output.push_str(&text);
                at += length;
                continue;
            }
        }
        let character = tail.chars().next().expect("nonempty suffix");
        output.push(character);
        at += character.len_utf8();
    }
    output
}
fn html(context: &mut Context, text: &str, source: &Source) -> Vec<Pair> {
    let lower = text.to_ascii_lowercase();
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut result = vec![];
    while at < bytes.len() && result.len() < 256 && context.step() {
        let Some(found) = text[at..].find('<') else {
            break;
        };
        at += found;
        if text[at..].starts_with("<!--") {
            at = text[at + 4..]
                .find("-->")
                .map_or(bytes.len(), |end| at + 4 + end + 3);
            continue;
        }
        let start = at;
        at += 1;
        let tag_start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_alphanumeric) {
            at += 1;
        }
        let tag = &lower[tag_start..at];
        if ["script", "style", "textarea", "title"].contains(&tag) {
            let close = format!("</{tag}");
            at = lower[at..]
                .find(&close)
                .map_or(bytes.len(), |end| at + end + close.len());
            continue;
        }
        let mut attributes = vec![];
        while at < bytes.len() && bytes[at] != b'>' && attributes.len() < 128 {
            while bytes
                .get(at)
                .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/')
            {
                at += 1;
            }
            if bytes.get(at) == Some(&b'>') {
                break;
            }
            let key_start = at;
            while bytes
                .get(at)
                .is_some_and(|b| !b.is_ascii_whitespace() && !b"=/>".contains(b))
            {
                at += 1;
            }
            if at == key_start {
                at += 1;
                continue;
            }
            let key = lower[key_start..at].to_owned();
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            let mut value = "";
            if bytes.get(at) == Some(&b'=') {
                at += 1;
                while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                    at += 1;
                }
                if let Some(quote @ (b'\'' | b'"')) = bytes.get(at).copied() {
                    at += 1;
                    let begin = at;
                    while bytes.get(at).is_some_and(|b| *b != quote) {
                        at += 1;
                    }
                    value = &text[begin..at];
                    at = (at + 1).min(bytes.len());
                } else {
                    let begin = at;
                    while bytes
                        .get(at)
                        .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                    {
                        at += 1;
                    }
                    value = &text[begin..at];
                }
            }
            attributes.push((key, entities(value)));
        }
        at = (at + 1).min(bytes.len());
        if attributes.len() >= 128 {
            context.coverage(
                "body",
                "limited",
                "HTML attribute budget exceeded; the remainder was not scanned.",
            );
            break;
        }
        if tag != "input" {
            continue;
        }
        if attributes
            .iter()
            .enumerate()
            .any(|(index, (name, _))| attributes[..index].iter().any(|(other, _)| other == name))
        {
            context.coverage("body", "incomplete", "Duplicate hidden-control attributes were retained in the original source; HTML first-attribute behavior was used.");
        }
        let attr = |name: &str| {
            attributes
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        if !attr("type").is_some_and(|v| v.eq_ignore_ascii_case("hidden")) {
            continue;
        }
        let Some(name) = attr("name") else {
            continue;
        };
        let mut loc = source.clone();
        loc.label = "HTML hidden form input".into();
        loc.occurrence = result.len();
        loc.offset = Some(start);
        loc.length = Some(at - start);
        loc.transforms
            .push("Inert HTML attribute/entity decoding".into());
        result.push(Pair {
            name: name.into(),
            value: attr("value").unwrap_or("").into(),
            original: text[start..at].into(),
            delimiter: attr("value").is_some(),
            source: loc,
        });
    }
    if result.len() >= 256 && at < bytes.len() {
        context.coverage(
            "body",
            "limited",
            "HTML hidden-control budget reached; remaining markup was not scanned.",
        );
    }
    result
}
