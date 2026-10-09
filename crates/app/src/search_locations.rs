//! Original-text positions for normalized search results, without a per-character
//! map allocation proportional to a large response body.
use super::{MAX_ENTRY_MATCHES, Matcher, TrafficSearchMatch, normalize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

impl Matcher {
    pub(super) fn locations(
        &self,
        text: &str,
        boundary: &str,
        field: &str,
        remaining: usize,
    ) -> Vec<TrafficSearchMatch> {
        let (normalized, case_sensitive, ignore_accents) = match self {
            Self::Text {
                case_sensitive,
                ignore_diacritics,
                ..
            } => (
                normalize(text, *case_sensitive, *ignore_diacritics),
                *case_sensitive,
                *ignore_diacritics,
            ),
            Self::Regex(_) => (text.to_owned(), true, false),
        };
        let ranges = match self {
            Self::Text { pattern, .. } => normalized
                .match_indices(pattern.as_str())
                .take(remaining.min(MAX_ENTRY_MATCHES + 1))
                .map(|(at, value)| (at, at + value.len()))
                .collect::<Vec<_>>(),
            Self::Regex(regex) => regex
                .find_iter(&normalized)
                .take(remaining.min(MAX_ENTRY_MATCHES + 1))
                .map(|found| (found.start(), found.end()))
                .collect::<Vec<_>>(),
        };
        if ranges.is_empty() {
            return Vec::new();
        }
        let mapped = original_ranges(text, &ranges, case_sensitive, ignore_accents);
        mapped
            .into_iter()
            .map(
                |OriginalRange {
                     start,
                     end,
                     start_utf16,
                     end_utf16,
                 }| {
                    let before = text[..start]
                        .chars()
                        .rev()
                        .take(80)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                    let matched = text[start..end].chars().take(160).collect();
                    let after = text[end..].chars().take(80).collect();
                    TrafficSearchMatch {
                        boundary: boundary.into(),
                        field: field.into(),
                        start_utf16,
                        end_utf16,
                        before,
                        matched,
                        after,
                        shortened: text[start..end].chars().nth(160).is_some(),
                    }
                },
            )
            .collect()
    }
}

#[derive(Clone, Copy, Default)]
struct OriginalRange {
    start: usize,
    end: usize,
    start_utf16: usize,
    end_utf16: usize,
}
fn original_ranges(
    text: &str,
    ranges: &[(usize, usize)],
    case_sensitive: bool,
    ignore_accents: bool,
) -> Vec<OriginalRange> {
    let mut mapped = vec![OriginalRange::default(); ranges.len()];
    let mut norm = 0;
    let mut utf16 = 0;
    let mut next_start = 0;
    let mut next_end = 0;
    for (offset, character) in text.char_indices() {
        let end = offset + character.len_utf8();
        let end_utf16 = utf16 + character.len_utf16();
        let size = if character.is_ascii() {
            1
        } else {
            let scalar = if ignore_accents {
                character
                    .to_string()
                    .nfd()
                    .filter(|character| !is_combining_mark(*character))
                    .collect::<String>()
            } else {
                character.to_string()
            };
            if case_sensitive {
                scalar.len()
            } else {
                scalar.to_lowercase().len()
            }
        };
        if size > 0 && next_start == ranges.len() && next_end == ranges.len() {
            break;
        }
        while next_start < ranges.len() && ranges[next_start].0 < norm + size {
            mapped[next_start].start = offset;
            mapped[next_start].start_utf16 = utf16;
            next_start += 1;
        }
        while next_end < ranges.len() && ranges[next_end].1 <= norm + size {
            let at_start = ranges[next_end].1 == norm;
            mapped[next_end].end = if at_start { offset } else { end };
            mapped[next_end].end_utf16 = if at_start { utf16 } else { end_utf16 };
            next_end += 1;
        }
        if size == 0 && ignore_accents {
            for (index, range) in ranges.iter().enumerate().take(next_end) {
                if range.0 < range.1 && range.1 == norm {
                    mapped[index].end = end;
                    mapped[index].end_utf16 = end_utf16;
                }
            }
        }
        norm += size;
        utf16 = end_utf16;
    }
    for row in mapped.iter_mut().skip(next_start) {
        row.start = text.len();
        row.start_utf16 = utf16;
    }
    for row in mapped.iter_mut().skip(next_end) {
        row.end = text.len();
        row.end_utf16 = utf16;
    }

    mapped
}

#[cfg(test)]
mod tests {
    use super::*;
    fn literal(pattern: &str, accents: bool) -> Matcher {
        Matcher::Text {
            pattern: normalize(pattern, false, accents),
            case_sensitive: false,
            ignore_diacritics: accents,
        }
    }
    #[test]
    fn highlights_original_composed_and_decomposed_accents_after_surrogate_pair() {
        let rows = literal("cafe", true).locations(
            "😀 Café / Cafe\u{301}",
            "client-response",
            "body",
            201,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].matched, "Café");
        assert_eq!((rows[0].start_utf16, rows[0].end_utf16), (3, 7));
        assert_eq!(rows[1].matched, "Cafe\u{301}");
        assert_eq!((rows[1].start_utf16, rows[1].end_utf16), (10, 15));
    }
    #[test]
    fn case_expansions_and_contextual_lowercase_map_back_to_original_scalars() {
        let rows = literal("i", false).locations("😀İ", "client-request", "header", 201);
        assert_eq!(rows[0].matched, "İ");
        assert_eq!((rows[0].start_utf16, rows[0].end_utf16), (2, 3));
        let rows = literal("ς", false).locations("ΟΣ", "metadata", "URL", 201);
        assert_eq!(rows[0].matched, "Σ");
        assert_eq!((rows[0].start_utf16, rows[0].end_utf16), (1, 2));
    }
    #[test]
    fn regex_zero_width_and_unicode_spans_use_original_utf16_positions() {
        let matcher = Matcher::Regex(regex::Regex::new("^|café|$").unwrap());
        let rows = matcher.locations("😀 café ", "client-response", "body", 201);
        assert_eq!(rows.len(), 3);
        assert_eq!((rows[0].start_utf16, rows[0].end_utf16), (0, 0));
        assert_eq!(rows[1].matched, "café");
        assert_eq!((rows[1].start_utf16, rows[1].end_utf16), (3, 7));
        assert_eq!((rows[2].start_utf16, rows[2].end_utf16), (8, 8));
    }
    #[test]
    fn occurrences_and_contexts_are_bounded_without_shortening_real_offsets() {
        let rows =
            literal("x", false).locations(&"x".repeat(10000), "client-response", "body", 201);
        assert_eq!(rows.len(), 201);
        let rows = Matcher::Regex(regex::Regex::new(".+").unwrap()).locations(
            &"😀".repeat(10000),
            "client-response",
            "body",
            201,
        );
        assert!(rows[0].shortened);
        assert_eq!(rows[0].matched.chars().count(), 160);
        assert_eq!(rows[0].end_utf16, 20000);
    }
}
