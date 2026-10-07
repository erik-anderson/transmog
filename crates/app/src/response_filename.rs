use percent_encoding::percent_decode_str;

pub(crate) fn suggested_filename(
    disposition: Option<&str>,
    path: &str,
    media_type: Option<&str>,
) -> String {
    if let Some(name) = disposition.and_then(disposition_filename) {
        return name;
    }
    let extensions = media_extensions(media_type.unwrap_or(""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let name = path
        .rsplit('/')
        .next()
        .and_then(decode_percent)
        .and_then(|value| sanitize_filename(&value));
    if let Some(name) = name {
        if let Some((stem, extension)) = name.rsplit_once('.') {
            if !stem.is_empty()
                && (extensions.is_empty()
                    || extensions
                        .iter()
                        .any(|candidate| extension.eq_ignore_ascii_case(candidate)))
            {
                return name;
            }
            if !stem.is_empty() {
                return format!("{stem}.{}", extensions.first().unwrap_or(&"bin"));
            }
        }
        return format!("{name}.{}", extensions.first().unwrap_or(&"bin"));
    }
    format!("response.{}", extensions.first().unwrap_or(&"bin"))
}

fn disposition_filename(header: &str) -> Option<String> {
    if header.len() > 8192 {
        return None;
    }
    let mut parts = Vec::new();
    let (mut quoted, mut escaped, mut start) = (false, false, 0);
    for (offset, character) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && character == '\\' {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ';' && !quoted {
            parts.push(&header[start..offset]);
            start = offset + 1;
        }
    }
    if quoted || escaped {
        return None;
    }
    parts.push(&header[start..]);
    let (mut plain, mut extended) = (None, None);
    let (mut plain_count, mut extended_count) = (0, 0);
    for part in parts.into_iter().skip(1) {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("filename*") {
            extended_count += 1;
            extended = parameter_value(value.trim());
        } else if name.trim().eq_ignore_ascii_case("filename") {
            plain_count += 1;
            plain = parameter_value(value.trim());
        }
    }
    let extended = if extended_count == 1 {
        extended
            .and_then(|value| extended_filename(&value))
            .and_then(|value| sanitize_filename(&value))
    } else {
        None
    };
    extended.or_else(|| {
        if plain_count == 1 {
            plain.and_then(|value| sanitize_filename(&value))
        } else {
            None
        }
    })
}

fn parameter_value(value: &str) -> Option<String> {
    if !value.starts_with('"') {
        return (!value.is_empty()).then(|| value.to_owned());
    }
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut result = String::new();
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character == '\\' {
            result.push(characters.next()?);
        } else if character == '"' {
            return None;
        } else {
            result.push(character);
        }
    }
    Some(result)
}

fn extended_filename(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let charset = parts.next()?;
    parts.next()?; // Language does not affect the filename bytes.
    let value = parts.next()?;
    validate_percent(value)?;
    if charset.eq_ignore_ascii_case("utf-8") {
        percent_decode_str(value)
            .decode_utf8()
            .ok()
            .map(std::borrow::Cow::into_owned)
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(percent_decode_str(value).map(char::from).collect())
    } else {
        None
    }
}

fn validate_percent(value: &str) -> Option<()> {
    let bytes = value.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'%' {
            let pair = bytes.get(offset + 1..offset + 3)?;
            if !pair.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            offset += 3;
        } else {
            offset += 1;
        }
    }
    Some(())
}

fn decode_percent(value: &str) -> Option<String> {
    validate_percent(value)?;
    percent_decode_str(value)
        .decode_utf8()
        .ok()
        .map(std::borrow::Cow::into_owned)
}

fn sanitize_filename(value: &str) -> Option<String> {
    let value = value.rsplit(['/', '\\']).next()?.trim();
    let mut name: String = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            {
                '_'
            } else {
                character
            }
        })
        .collect();
    name.truncate(name.trim_end_matches(['.', ' ']).len());
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    let device = name.split('.').next()?.to_ascii_uppercase();
    if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            device.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        })
    {
        name.insert(0, '_');
    }
    // Preserve the extension while keeping within Windows' component limit.
    if name.encode_utf16().count() > 240 {
        let (stem, suffix) = name
            .rsplit_once('.')
            .filter(|(_, extension)| extension.encode_utf16().count() <= 24)
            .map_or((name.as_str(), String::new()), |(stem, extension)| {
                (stem, format!(".{extension}"))
            });
        let limit = 240 - suffix.encode_utf16().count();
        let mut length = 0;
        name = stem
            .chars()
            .take_while(|character| {
                length += character.len_utf16();
                length <= limit
            })
            .collect::<String>()
            + suffix.as_str();
    }
    Some(name)
}

fn media_extensions(media_type: &str) -> &'static [&'static str] {
    let mime = media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.as_str() {
        "image/png" => &["png"],
        "image/jpeg" => &["jpg", "jpeg", "jfif"],
        "image/webp" => &["webp"],
        "image/gif" => &["gif"],
        "image/svg+xml" => &["svg"],
        "image/avif" => &["avif"],
        "image/bmp" => &["bmp"],
        "image/tiff" => &["tiff", "tif"],
        "image/x-icon" | "image/vnd.microsoft.icon" => &["ico"],
        "text/html" => &["html", "htm"],
        "text/plain" => &["txt"],
        "text/css" => &["css"],
        "text/markdown" => &["md"],
        "text/javascript" | "application/javascript" => &["js"],
        "application/pdf" => &["pdf"],
        "application/json" => &["json"],
        "text/xml" | "application/xml" => &["xml"],
        "application/wasm" => &["wasm"],
        "application/zip" => &["zip"],
        "application/gzip" => &["gz"],
        "audio/mpeg" => &["mp3"],
        "audio/wav" | "audio/x-wav" => &["wav"],
        "audio/ogg" => &["ogg"],
        "video/mp4" => &["mp4"],
        "video/webm" => &["webm"],
        _ if mime.ends_with("+json") => &["json"],
        _ if mime.ends_with("+xml") => &["xml"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition_precedence_encoding_and_quoted_values() {
        for (header, expected) in [
            (
                "inline;filename*=UTF-8''Jack_of_the_United_States.svg.webp",
                "Jack_of_the_United_States.svg.webp",
            ),
            (
                "attachment; filename=legacy.txt; filename*=UTF-8'en'%E2%82%AC%20rates.txt",
                "€ rates.txt",
            ),
            (
                "attachment; FILENAME*=utf-8''C%2B%2B.txt; filename=old.txt",
                "C++.txt",
            ),
            ("attachment; filename*=ISO-8859-1''caf%E9.txt", "café.txt"),
            (
                "inline; filename*=UTF-8''%FF; filename=valid.txt",
                "valid.txt",
            ),
            (
                "inline; filename*=UTF-8''bad%GG; filename=valid.txt",
                "valid.txt",
            ),
            (
                "inline; filename*=unsupported''name; filename=valid.txt",
                "valid.txt",
            ),
            ("inline; filename=\"semi;colon.txt\"", "semi;colon.txt"),
            ("inline; filename=\"a\\\"b.txt\"", "a_b.txt"),
            (
                "inline; filename*=UTF-8''one; filename*=UTF-8''two; filename=valid.txt",
                "valid.txt",
            ),
        ] {
            assert_eq!(
                suggested_filename(Some(header), "/", Some("text/plain")),
                expected,
                "{header}"
            );
        }
    }

    #[test]
    fn safe_names_and_url_media_fallbacks() {
        for (header, path, mime, expected) in [
            (
                Some("inline; filename*=UTF-8''..%2F..%2Fpic.png"),
                "/",
                "image/png",
                "pic.png",
            ),
            (
                Some("inline; filename=CON.txt"),
                "/",
                "text/plain",
                "_CON.txt",
            ),
            (
                Some("inline; filename=\"../\""),
                "/icons/logo.png?key=secret",
                "image/png",
                "logo.png",
            ),
            (None, "/icons/a%20b.PNG?key=secret", "image/png", "a b.PNG"),
            (None, "/images/get.php", "image/png", "get.png"),
            (
                None,
                "/api/report",
                "application/problem+json",
                "report.json",
            ),
            (None, "/", "image/webp", "response.webp"),
            (None, "/", "application/octet-stream", "response.bin"),
            (
                None,
                "/bundle.tar.gz",
                "application/octet-stream",
                "bundle.tar.gz",
            ),
        ] {
            assert_eq!(suggested_filename(header, path, Some(mime)), expected);
        }
        let long = format!("{}.png", "😀".repeat(200));
        let name = sanitize_filename(&long).unwrap();
        assert!(name.encode_utf16().count() <= 240);
        assert_eq!(std::path::Path::new(&name).extension().unwrap(), "png");
    }
}
