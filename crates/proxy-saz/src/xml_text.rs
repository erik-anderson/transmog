//! Reversible display text for characters that XML 1.0 cannot represent.
use crate::SazError;
use std::{collections::BTreeMap, fmt::Write as _};

pub(crate) const ATTRIBUTE: &str = "TransmogXmlCharacterProjection";
pub(crate) const FLAG: &str = "x-transmog-xml-character-projection";
pub(crate) const MODE: &str = "unicode-escapes";
pub(crate) const MAX_EXPANSION: usize = 8;

pub(crate) fn marked(flags: &BTreeMap<String, String>) -> bool {
    flags.get(FLAG).is_some_and(|mode| mode == MODE)
}

fn xml_character(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\r'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}'
    )
}

pub(crate) fn needs_projection(value: &str) -> bool {
    value.chars().any(|character| !xml_character(character))
}

/// Projects all attribute values when the document declares this encoding.
pub(crate) fn project(value: &str) -> String {
    let mut displayed = String::new();
    for character in value.chars() {
        if !xml_character(character) {
            let _ = write!(displayed, "\\u{{{:04x}}}", u32::from(character));
        } else if character == '\\' {
            displayed.push_str("\\\\");
        } else {
            displayed.push(character);
        }
    }
    displayed
}

/// Used only for generated, readable diagnostics; no capture facts are mutated.
pub(crate) fn display_value(value: &str) -> String {
    if needs_projection(value) {
        project(value)
    } else {
        value.to_owned()
    }
}

pub(crate) fn restore(value: &str) -> Result<String, SazError> {
    let mut restored = String::new();
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            if !xml_character(character) {
                return Err(SazError::InvalidArchive);
            }
            restored.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => restored.push('\\'),
            Some('u') if characters.next() == Some('{') => {
                let mut codepoint = 0;
                for _ in 0..4 {
                    let digit = characters
                        .next()
                        .and_then(|character| character.to_digit(16))
                        .ok_or(SazError::InvalidArchive)?;
                    codepoint = codepoint * 16 + digit;
                }
                let character = char::from_u32(codepoint)
                    .filter(|character| !xml_character(*character))
                    .ok_or(SazError::InvalidArchive)?;
                if characters.next() != Some('}') {
                    return Err(SazError::InvalidArchive);
                }
                restored.push(character);
            }
            _ => return Err(SazError::InvalidArchive),
        }
    }
    // The encoded allowance must not inflate retained metadata allocations.
    restored.shrink_to_fit();
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_are_reversible_and_bounded() {
        let source = "literal\\u{0000}\\\0\u{1f}\t\r\n&<>\"'\u{fffe}\u{ffff}🦀";
        let projected = project(source);
        assert_eq!(restore(&projected).unwrap(), source);
        assert!(!needs_projection(&projected));
        assert!(projected.len() <= source.len() * MAX_EXPANSION);
        assert_eq!(display_value("X25519\0MLKEM768"), "X25519\\u{0000}MLKEM768");
        assert_eq!(project("\0"), "\\u{0000}");
        assert_eq!(display_value("literal\\u{0000}"), "literal\\u{0000}");
    }

    #[test]
    fn malformed_or_noncanonical_projections_are_rejected() {
        for value in [
            "\\",
            "\\n",
            "\\u{}",
            "\\u{000}",
            "\\u{00000}",
            "\\u{d800}",
            "\\u{0061}",
            "\\u{zzzz}",
            "\0",
        ] {
            assert!(restore(value).is_err(), "accepted {value:?}");
        }
    }
}
