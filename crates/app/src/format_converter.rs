//! Bounded, local text and byte conversions, independent of desktop windows.

use std::sync::atomic::{AtomicBool, Ordering};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use boring::hash::{Hasher, MessageDigest};
use serde::{Deserialize, Serialize};
pub use transmog_auth::{AUTH_CONVERSIONS, AuthConversion};

/// Maximum decoded input bytes.
pub const MAX_INPUT: usize = 8 * 1024 * 1024;
/// Maximum produced bytes.
pub const MAX_OUTPUT: usize = 32 * 1024 * 1024;

/// Explicit temporary scratch handoff; never persist this credential-bearing text.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConverterText {
    /// Complete original text or a lossless explicit byte representation.
    pub text: String,
    /// Native catalog operation hint, when the evidence supplies one.
    pub operation: Option<String>,
}

/// Representation of bytes in an editor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Representation {
    /// Strict text in the chosen encoding.
    #[default]
    Text,
    /// Hexadecimal byte pairs with optional ASCII whitespace between pairs.
    Hex,
}

/// Explicit encoding for text and byte boundaries; BOMs are ordinary content.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextEncoding {
    /// UTF-8 without replacement.
    #[default]
    Utf8,
    /// Little-endian UTF-16 without an implicit BOM.
    Utf16le,
    /// Big-endian UTF-16 without an implicit BOM.
    Utf16be,
    /// Windows Western European encoding.
    Windows1252,
}

/// Built-in conversion, without custom code execution.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    /// Standard Base64 encoding.
    Base64Encode,
    /// Standard Base64 decoding.
    Base64Decode,
    /// Unpadded URL-safe Base64 encoding.
    Base64urlEncode,
    /// URL-safe Base64 decoding.
    Base64urlDecode,
    /// RFC 3986 component escaping.
    UrlEncode,
    /// Strict component unescaping; plus remains plus.
    UrlDecode,
    /// Form value escaping; spaces become plus.
    FormEncode,
    /// Form value unescaping; plus becomes space.
    FormDecode,
    /// Lowercase hexadecimal encoding.
    HexEncode,
    /// Hexadecimal decoding.
    HexDecode,
    /// HTML text and attribute character escaping.
    HtmlEncode,
    /// HTML character reference decoding as inert text.
    HtmlDecode,
    /// JSON string quoting.
    JsonQuote,
    /// Strict JSON string unquoting.
    JsonUnquote,
    /// Detect and decode a supported authentication header or standalone token.
    AuthDecode,
    /// Decode a general compact JWS, including binary payloads.
    JwsDecode,
    /// Inspect encrypted compact JWE metadata.
    JweInspect,
    /// Decode Basic credentials without guessing their character set.
    BasicDecode,
    /// Parse ordered Digest parameters.
    DigestParse,
    /// Decode an NTLM mechanism token.
    NtlmDecode,
    /// Decode GSS/SPNEGO and confirmed nested mechanisms.
    NegotiateDecode,
    /// Decode a compact JWT as unverified JSON header, payload and signature.
    JwtDecode,
    /// MD5 digest.
    Md5,
    /// SHA-1 digest.
    Sha1,
    /// SHA-256 digest.
    Sha256,
    /// SHA-384 digest.
    Sha384,
    /// SHA-512 digest.
    Sha512,
}
impl Operation {
    fn authentication(self) -> bool {
        matches!(
            self,
            Self::AuthDecode
                | Self::JwtDecode
                | Self::JwsDecode
                | Self::JweInspect
                | Self::BasicDecode
                | Self::DigestParse
                | Self::NtlmDecode
                | Self::NegotiateDecode
        )
    }
}

/// A complete conversion snapshot. Never persist or log its contents.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionInput {
    /// Authored input, with no implicit whitespace trimming.
    pub input: String,
    /// Interpretation of the input editor.
    pub representation: Representation,
    /// Text and byte boundary encoding.
    pub encoding: TextEncoding,
    /// Selected operation.
    pub operation: Operation,
    /// Preferred output presentation; invalid text falls back to hex.
    pub output_representation: Representation,
}

/// Display result; original bytes remain available for an explicit save.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionOutput {
    /// Decoded authentication JSON needs an explicitly chosen next conversion when chained.
    pub requires_next_conversion: bool,
    /// Encoding that preserves the result bytes when chaining text output.
    pub encoding: TextEncoding,
    /// Complete displayed value.
    pub display: String,
    /// Actual display representation.
    pub representation: Representation,
    /// Whether the bytes can be displayed as strict text.
    pub text_available: bool,
    /// Complete output byte count.
    pub byte_length: usize,
    /// Complete output bytes, excluded from desktop JSON replies.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// Converts a snapshot with cooperative cancellation between bounded stages.
///
/// # Errors
/// Returns content-free validation, cancellation, or resource-limit messages.
pub fn convert(
    input: &ConversionInput,
    cancelled: &AtomicBool,
) -> Result<ConversionOutput, String> {
    check_cancelled(cancelled)?;
    if input.input.len() > MAX_INPUT * 3 {
        return Err("Input exceeds the 8 MiB byte limit.".into());
    }
    let unicode_operation = matches!(
        input.operation,
        Operation::UrlEncode
            | Operation::UrlDecode
            | Operation::FormEncode
            | Operation::FormDecode
            | Operation::HtmlEncode
            | Operation::HtmlDecode
            | Operation::JsonQuote
            | Operation::JsonUnquote
            | Operation::JwtDecode
            | Operation::AuthDecode
            | Operation::JwsDecode
            | Operation::JweInspect
            | Operation::BasicDecode
            | Operation::DigestParse
            | Operation::NtlmDecode
            | Operation::NegotiateDecode
    );
    let bytes = match input.representation {
        Representation::Text => encode_text(
            &input.input,
            if unicode_operation {
                TextEncoding::Utf8
            } else {
                input.encoding
            },
        )?,
        Representation::Hex => parse_hex(&input.input, cancelled)?,
    };
    if bytes.len() > MAX_INPUT {
        return Err("Input exceeds the 8 MiB byte limit.".into());
    }
    check_cancelled(cancelled)?;
    let literal_text = || match input.representation {
        Representation::Text => Ok(input.input.clone()),
        Representation::Hex => decode_text(&bytes, input.encoding),
    };
    let mut text_result = true;
    let result = match input.operation {
        Operation::Base64Encode => STANDARD.encode(&bytes).into_bytes(),
        Operation::Base64urlEncode => URL_SAFE_NO_PAD.encode(&bytes).into_bytes(),
        Operation::Base64Decode | Operation::Base64urlDecode => {
            text_result = false;
            let text = literal_text()?;
            let (padded, unpadded) = if matches!(input.operation, Operation::Base64Decode) { (STANDARD, STANDARD_NO_PAD) } else { (URL_SAFE, URL_SAFE_NO_PAD) };
            padded.decode(&text).or_else(|_| unpadded.decode(&text)).map_err(|_| "Invalid Base64: check the alphabet, padding and unused bits; whitespace is not accepted.".to_owned())?
        }
        Operation::HexEncode => hex(&bytes).into_bytes(),
        Operation::HexDecode => { text_result = false; parse_hex(&literal_text()?, cancelled)? }
        Operation::UrlEncode | Operation::FormEncode => {
            escape_url(&literal_text()?, matches!(input.operation, Operation::FormEncode), cancelled)?.into_bytes()
        }
        Operation::UrlDecode | Operation::FormDecode => {
            unescape_url(&literal_text()?, matches!(input.operation, Operation::FormDecode), cancelled)?.into_bytes()
        }
        Operation::HtmlEncode => {
            let text = literal_text()?;
            let mut output = String::new();
            for (index, character) in text.chars().enumerate() {
                if index % 4096 == 0 { check_cancelled(cancelled)?; }
                match character {
                    '&' => output.push_str("&amp;"), '<' => output.push_str("&lt;"), '>' => output.push_str("&gt;"),
                    '"' => output.push_str("&quot;"), '\'' => output.push_str("&#39;"), _ => output.push(character),
                }
                check_output(output.len())?;
            }
            output.into_bytes()
        }
        Operation::HtmlDecode => unescape_html(&literal_text()?, cancelled)?.into_bytes(),
        Operation::JsonQuote => serde_json::to_string(&literal_text()?).map_err(|_| "JSON quoting failed.")?.into_bytes(),
        Operation::JsonUnquote => serde_json::from_str::<String>(&literal_text()?).map_err(|_| "Expected a complete JSON string with valid escapes and paired Unicode surrogates.")?.into_bytes(),
        operation @ (Operation::JwtDecode | Operation::AuthDecode | Operation::JwsDecode | Operation::JweInspect | Operation::BasicDecode | Operation::DigestParse | Operation::NtlmDecode | Operation::NegotiateDecode) => {
            let id = serde_json::to_value(operation).map_err(|_| "Unsupported authentication conversion.")?;
            transmog_auth::decode_authentication(&literal_text()?, id.as_str().ok_or("Unsupported authentication conversion.")?, cancelled)?.into_bytes()
        },
        operation => {
            let digest = match operation {
                Operation::Md5 => MessageDigest::md5(), Operation::Sha1 => MessageDigest::sha1(),
                Operation::Sha384 => MessageDigest::sha384(), Operation::Sha512 => MessageDigest::sha512(),
                _ => MessageDigest::sha256(),
            };
            let mut hasher = Hasher::new(digest).map_err(|_| "Hash initialization failed.")?;
            for chunk in bytes.chunks(64 * 1024) {
                check_cancelled(cancelled)?;
                hasher.update(chunk).map_err(|_| "Hashing failed.")?;
            }
            hex(&hasher.finish().map_err(|_| "Hashing failed.")?).into_bytes()
        }
    };
    check_cancelled(cancelled)?;
    finish_conversion(input, result, text_result, cancelled)
}

fn finish_conversion(
    input: &ConversionInput,
    result: Vec<u8>,
    text_result: bool,
    cancelled: &AtomicBool,
) -> Result<ConversionOutput, String> {
    check_output(result.len())?;
    let text = if text_result {
        String::from_utf8(result.clone()).map_err(|_| "Text conversion failed.".to_owned())
    } else {
        decode_text(&result, input.encoding)
    };
    let text = text.and_then(|text| {
        if text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\t' | '\n' | '\r'))
        {
            Err("Output contains control bytes. Use Hex output.".to_owned())
        } else {
            Ok(text)
        }
    });
    let text_available = text.is_ok();
    let representation = if input.output_representation == Representation::Hex || !text_available {
        Representation::Hex
    } else {
        Representation::Text
    };
    let display = if representation == Representation::Hex {
        hex(&result)
    } else {
        text?
    };
    check_cancelled(cancelled)?;
    Ok(ConversionOutput {
        requires_next_conversion: input.operation.authentication(),
        encoding: if text_result {
            TextEncoding::Utf8
        } else {
            input.encoding
        },
        display,
        representation,
        text_available,
        byte_length: result.len(),
        bytes: result,
    })
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Conversion canceled.".into())
    } else {
        Ok(())
    }
}

fn check_output(length: usize) -> Result<(), String> {
    if length > MAX_OUTPUT {
        Err("Output exceeds the 32 MiB byte limit.".into())
    } else {
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    output
}

fn parse_hex(text: &str, cancelled: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut high = None;
    for (index, byte) in text.bytes().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(cancelled)?;
        }
        if byte.is_ascii_whitespace() && high.is_none() {
            continue;
        }
        let digit = char::from(byte)
            .to_digit(16)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or("Hex requires complete byte pairs; whitespace is allowed only between pairs.")?;
        if let Some(previous) = high.take() {
            output.push(previous * 16 + digit);
        } else {
            high = Some(digit);
        }
        if output.len() > MAX_INPUT {
            return Err("Input exceeds the 8 MiB byte limit.".into());
        }
    }
    if high.is_some() {
        return Err("Hex requires an even number of digits.".into());
    }
    Ok(output)
}

const WESTERN: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

fn encode_text(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, String> {
    match encoding {
        TextEncoding::Utf8 => Ok(text.as_bytes().to_vec()),
        TextEncoding::Utf16le | TextEncoding::Utf16be => Ok(text
            .encode_utf16()
            .flat_map(|unit| {
                if matches!(encoding, TextEncoding::Utf16le) {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                }
            })
            .collect()),
        TextEncoding::Windows1252 => text
            .chars()
            .map(|character| {
                let point = u32::from(character);
                if point < 128 || (160..=255).contains(&point) {
                    u8::try_from(point)
                        .map_err(|_| "Character cannot be encoded as Windows-1252.".into())
                } else {
                    WESTERN
                        .iter()
                        .position(|value| *value == character)
                        .and_then(|index| u8::try_from(index + 128).ok())
                        .ok_or_else(|| "Character cannot be encoded as Windows-1252.".into())
                }
            })
            .collect(),
    }
}

fn decode_text(bytes: &[u8], encoding: TextEncoding) -> Result<String, String> {
    match encoding {
        TextEncoding::Utf8 => {
            String::from_utf8(bytes.to_vec()).map_err(|_| "Bytes are not valid UTF-8.".into())
        }
        TextEncoding::Utf16le | TextEncoding::Utf16be => {
            if !bytes.len().is_multiple_of(2) {
                return Err("UTF-16 requires complete two-byte units.".into());
            }
            let units: Vec<_> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    if matches!(encoding, TextEncoding::Utf16le) {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    }
                })
                .collect();
            String::from_utf16(&units)
                .map_err(|_| "Bytes contain unpaired UTF-16 surrogates.".into())
        }
        TextEncoding::Windows1252 => Ok(bytes
            .iter()
            .map(|byte| {
                if (128..160).contains(byte) {
                    WESTERN[usize::from(*byte - 128)]
                } else {
                    char::from(*byte)
                }
            })
            .collect()),
    }
}

fn unescape_html(text: &str, cancelled: &AtomicBool) -> Result<String, String> {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < text.len() {
        check_cancelled(cancelled)?;
        let remaining = &text[cursor..];
        if !remaining.starts_with('&') {
            let count = remaining.find('&').unwrap_or(remaining.len());
            output.push_str(&remaining[..count]);
            cursor += count;
            continue;
        }
        if let Some(end) = remaining.bytes().take(34).position(|byte| byte == b';') {
            let entity = &remaining[1..=end];
            if let Some(number) = entity.strip_prefix('#') {
                let digits = number.trim_end_matches(';');
                let point = if let Some(hex) = digits
                    .strip_prefix('x')
                    .or_else(|| digits.strip_prefix('X'))
                {
                    u32::from_str_radix(hex, 16)
                } else {
                    digits.parse::<u32>()
                }
                .map_err(|_| "Invalid numeric HTML character reference.")?;
                let character = char::from_u32(point)
                    .filter(|character| *character != '\0')
                    .ok_or("Invalid Unicode value in HTML character reference.")?;
                output.push(character);
                cursor += end + 1;
                continue;
            }
            if let Some(&(first, second)) = web_atoms::NAMED_ENTITIES.get(entity)
                && let Some(character) = char::from_u32(first).filter(|_| first != 0)
            {
                output.push(character);
                if let Some(character) = char::from_u32(second).filter(|_| second != 0) {
                    output.push(character);
                }
                cursor += end + 1;
                continue;
            }
        }
        output.push('&');
        cursor += 1;
    }
    Ok(output)
}

fn escape_url(text: &str, form: bool, cancelled: &AtomicBool) -> Result<String, String> {
    let mut output = String::new();
    for (index, byte) in text.bytes().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(cancelled)?;
        }
        if byte.is_ascii_alphanumeric()
            || b"-._".contains(&byte)
            || byte == if form { b'*' } else { b'~' }
        {
            output.push(char::from(byte));
        } else if form && byte == b' ' {
            output.push('+');
        } else {
            const DIGITS: &[u8] = b"0123456789ABCDEF";
            output.push('%');
            output.push(char::from(DIGITS[usize::from(byte >> 4)]));
            output.push(char::from(DIGITS[usize::from(byte & 15)]));
        }
        check_output(output.len())?;
    }
    Ok(output)
}

fn unescape_url(text: &str, form: bool, cancelled: &AtomicBool) -> Result<String, String> {
    let mut output = Vec::new();
    let mut bytes = text.bytes().enumerate();
    while let Some((index, byte)) = bytes.next() {
        if index % 4096 == 0 {
            check_cancelled(cancelled)?;
        }
        if byte == b'%' {
            let high = bytes
                .next()
                .and_then(|(_, value)| char::from(value).to_digit(16));
            let low = bytes
                .next()
                .and_then(|(_, value)| char::from(value).to_digit(16));
            let value = high
                .zip(low)
                .and_then(|(high, low)| u8::try_from(high * 16 + low).ok())
                .ok_or("URL escape must contain two hexadecimal digits.")?;
            output.push(value);
        } else {
            output.push(if form && byte == b'+' { b' ' } else { byte });
        }
    }
    String::from_utf8(output).map_err(|_| "URL contains bytes that are not valid UTF-8.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(operation: Operation, text: &str) -> ConversionInput {
        ConversionInput {
            input: text.into(),
            representation: Representation::Text,
            encoding: TextEncoding::Utf8,
            operation,
            output_representation: Representation::Text,
        }
    }
    fn run(operation: Operation, text: &str) -> ConversionOutput {
        convert(&request(operation, text), &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn round_trips_preserve_unicode_whitespace_and_empty_input() {
        for text in ["", " \r\n\t", "héllo 🦊"] {
            for (encode, decode) in [
                (Operation::Base64Encode, Operation::Base64Decode),
                (Operation::Base64urlEncode, Operation::Base64urlDecode),
                (Operation::HexEncode, Operation::HexDecode),
                (Operation::UrlEncode, Operation::UrlDecode),
                (Operation::FormEncode, Operation::FormDecode),
                (Operation::HtmlEncode, Operation::HtmlDecode),
                (Operation::JsonQuote, Operation::JsonUnquote),
            ] {
                assert_eq!(run(decode, &run(encode, text).display).display, text);
            }
        }
        assert_eq!(run(Operation::UrlDecode, "a+b").display, "a+b");
        assert_eq!(run(Operation::FormDecode, "a+b").display, "a b");
        assert_eq!(run(Operation::HtmlDecode, "&NotEqualTilde;").display, "≂̸");
    }

    #[test]
    fn known_digests_and_binary_output() {
        for (operation, expected) in [
            (Operation::Md5, "900150983cd24fb0d6963f7d28e17f72"),
            (Operation::Sha1, "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                Operation::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                Operation::Sha384,
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                Operation::Sha512,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
        ] {
            assert_eq!(run(operation, "abc").display, expected);
        }
        let output = run(Operation::Base64Decode, "/wA=");
        assert_eq!(output.representation, Representation::Hex);
        assert_eq!(output.display, "ff00");
        assert!(!output.text_available);
        assert_eq!(run(Operation::HexDecode, "ff 00\n").bytes, output.bytes);
        let controls = run(Operation::Base64Decode, "AAE=");
        assert_eq!(controls.display, "0001");
        assert_eq!(controls.representation, Representation::Hex);
    }

    #[test]
    fn jwt_conversion_emits_utf8_json_and_preserves_output_bytes() {
        let token = "eyJhbGciOiJub25lIn0.eyJzdWIiOiJ1c2VyIn0.";
        let mut input = request(Operation::JwtDecode, token);
        input.encoding = TextEncoding::Utf16be;
        let output = convert(&input, &AtomicBool::new(false)).unwrap();
        let decoded: serde_json::Value = serde_json::from_str(&output.display).unwrap();
        assert!(output.requires_next_conversion);
        assert_eq!(decoded["payload"]["sub"], "user");
        assert_eq!(decoded["verification"], "Not checked");
        assert!(matches!(output.encoding, TextEncoding::Utf8));
        assert_eq!(output.bytes, output.display.as_bytes());
        input.representation = Representation::Hex;
        input.input = hex(&encode_text(token, input.encoding).unwrap());
        input.output_representation = Representation::Hex;
        let binary = convert(&input, &AtomicBool::new(false)).unwrap();
        assert_eq!(binary.bytes, output.bytes);
        assert_eq!(binary.display, hex(&output.bytes));
        assert!(
            convert(
                &request(Operation::JwtDecode, "private-not-a-token"),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            convert(
                &request(Operation::JwtDecode, token),
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }

    #[test]
    fn authentication_catalog_dispatch_and_unicode_output_are_in_sync() {
        for conversion in AUTH_CONVERSIONS {
            let operation: Operation =
                serde_json::from_value(serde_json::json!(conversion.id)).unwrap();
            assert_eq!(serde_json::to_value(operation).unwrap(), conversion.id);
        }
        for (operation, input, expected) in [
            (
                Operation::AuthDecode,
                "Authorization: Bearer opaque",
                "Bearer",
            ),
            (Operation::BasicDecode, "dXNlcjpwYXNz", "Basic"),
            (
                Operation::DigestParse,
                "realm=\"a,b\", nonce=\"one\", nonce=\"two\"",
                "Digest",
            ),
            (
                Operation::JweInspect,
                "eyJhbGciOiJkaXIiLCJlbmMiOiJBMTI4R0NNIn0..aXY.Y2lwaGVy.dGFn",
                "JWE",
            ),
            (Operation::JwsDecode, "eyJhbGciOiJub25lIn0./w.", "JWS"),
        ] {
            // Use the proper JOSE Base64url alphabet for the binary fixture.
            let input = input.replace("/w", "_w");
            let mut input = request(operation, &input);
            input.encoding = TextEncoding::Utf16be;
            let output = convert(&input, &AtomicBool::new(false)).unwrap();
            let value: serde_json::Value = serde_json::from_str(&output.display).unwrap();
            assert!(output.requires_next_conversion);
            assert_eq!(value["format"], expected);
            assert_eq!(output.display.as_bytes(), output.bytes);
            assert!(matches!(output.encoding, TextEncoding::Utf8));
        }
    }

    #[test]
    fn malformed_input_is_rejected_without_echoing_contents() {
        for (operation, text) in [
            (Operation::HexDecode, "f f"),
            (Operation::HexDecode, "123"),
            (Operation::Base64Decode, "Zh=="),
            (Operation::Base64Decode, "Zg==\n"),
            (Operation::UrlDecode, "%x0"),
            (Operation::UrlDecode, "%ff"),
            (Operation::JsonUnquote, "\"\\uD800\""),
        ] {
            assert!(convert(&request(operation, text), &AtomicBool::new(false)).is_err());
        }
        assert_eq!(run(Operation::Base64Decode, "Zg").display, "f");
    }

    #[test]
    fn encodings_boms_limits_and_cancellation() {
        for encoding in [
            TextEncoding::Utf8,
            TextEncoding::Utf16le,
            TextEncoding::Utf16be,
            TextEncoding::Windows1252,
        ] {
            assert_eq!(
                decode_text(&encode_text("€ A", encoding).unwrap(), encoding).unwrap(),
                "€ A"
            );
        }
        assert!(encode_text("🦊", TextEncoding::Windows1252).is_err());
        assert!(decode_text(&[0, 216], TextEncoding::Utf16le).is_err());
        assert!(decode_text(&[0], TextEncoding::Utf16le).is_err());
        assert_eq!(
            decode_text(&[255, 254, 65, 0], TextEncoding::Utf16le).unwrap(),
            "\u{feff}A"
        );
        assert!(convert(&request(Operation::Sha256, "a"), &AtomicBool::new(true)).is_err());
        assert!(
            convert(
                &request(Operation::Sha256, &"a".repeat(MAX_INPUT + 1)),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            convert(
                &request(Operation::HtmlEncode, &"\"".repeat(MAX_INPUT)),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn byte_encodings_preserve_chaining_and_do_not_change_unicode_escaping() {
        let mut input = request(Operation::Base64Encode, "A");
        input.encoding = TextEncoding::Utf16le;
        let encoded = convert(&input, &AtomicBool::new(false)).unwrap();
        assert!(!encoded.requires_next_conversion);
        assert_eq!(encoded.display, "QQA=");
        assert!(matches!(encoded.encoding, TextEncoding::Utf8));
        input.operation = Operation::Base64Decode;
        input.input = encoded.display;
        let decoded = convert(&input, &AtomicBool::new(false)).unwrap();
        assert_eq!(decoded.display, "A");
        assert_eq!(decoded.bytes, [65, 0]);
        assert!(matches!(decoded.encoding, TextEncoding::Utf16le));
        input.operation = Operation::UrlEncode;
        input.encoding = TextEncoding::Windows1252;
        input.input = "🦊".into();
        assert_eq!(
            convert(&input, &AtomicBool::new(false)).unwrap().display,
            "%F0%9F%A6%8A"
        );
        assert_eq!(
            run(Operation::HtmlDecode, "&unknown; &amp").display,
            "&unknown; &amp"
        );
        assert!(
            convert(
                &request(Operation::HtmlDecode, "&#0;"),
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }
}
