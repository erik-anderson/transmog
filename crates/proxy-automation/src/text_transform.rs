//! Bounded literal operations reuse the ordinary decoded buffering hook path.
use crate::{MatchCheck, MatchTest, Rule, compile_predicates, test_matcher};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use transmog_core::{
    HeaderBlock, RequestHead, ResponseHead,
    intercept::{
        BodyEditReport, BodyHookError, BodyPlan, BoxBodyFuture, BufferedBody, BufferedBodyHandler,
        ResponseBodyAction,
    },
};

/// Which non-overlapping literal occurrences are replaced.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Occurrences {
    /// Replace only the first match.
    First,
    /// Replace every match.
    All,
}
/// One explicit UTF-8 literal operation on a complete fresh response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiteralTransform {
    /// Nonempty case-sensitive literal; never a regex.
    pub find: String,
    /// Literal replacement, including an empty string for deletion.
    pub replacement: String,
    /// First or all non-overlapping occurrences.
    pub occurrences: Occurrences,
    /// Required total match count before First/All selection; mismatch fails.
    pub expected_matches: Option<usize>,
    /// Explicit exact base media type, excluding parameters.
    pub media_type: String,
    /// Explicit character encoding; this increment supports only utf-8.
    pub encoding: String,
    /// Complete decoded input and output bound, at most 1 MiB.
    pub max_bytes: usize,
}
/// Independent draft test using production conditions and literal execution.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextTransformTest {
    /// Request, response and media-type checks.
    pub test: MatchTest,
    /// Redacted operation outcome; absent when conditions do not match.
    pub report: Option<BodyEditReport>,
    /// Explicit supplied sample, rendered as inert text.
    pub before_text: String,
    /// Resulting sample, unchanged when not matched or failed.
    pub after_text: String,
}
/// Recognizes only standalone fresh-response literal rules.
pub fn is_text_transform_rule(rule: &Rule) -> bool {
    rule.response.text_transform.is_some()
        && rule.request.is_empty()
        && rule.response.headers.is_empty()
        && rule.response.replace_body.is_none()
        && !rule.response.discard_body
        && rule.response.abort_reason.is_none()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn pending() -> BodyEditReport {
    BodyEditReport {
        outcome: "not-completed".into(),
        matches: None,
        replacements: 0,
        before_bytes: None,
        after_bytes: None,
        before_sha256: None,
        after_sha256: None,
    }
}
impl LiteralTransform {
    pub(super) fn validate(&self) -> Result<(), ()> {
        if self.find.is_empty()
            || self.find.len() > 16 * 1024
            || self.replacement.len() > 16 * 1024
            || !(1..=1024 * 1024).contains(&self.max_bytes)
            || self.encoding != "utf-8"
            || self.expected_matches.is_some_and(|n| n > 1024 * 1024)
            || !valid_media(&self.media_type)
        {
            return Err(());
        }
        Ok(())
    }
    pub(super) fn media_matches(&self, headers: &HeaderBlock) -> bool {
        headers.values("content-type").any(|value| {
            std::str::from_utf8(value).is_ok_and(|value| {
                value
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case(&self.media_type)
            })
        })
    }
    pub(super) fn body_action(
        &self,
        head: &ResponseHead,
        id: &str,
        revision: u64,
    ) -> ResponseBodyAction {
        ResponseBodyAction::decoded(BodyPlan::Buffer {
            limit: NonZeroUsize::new(self.max_bytes).expect("validated bound"),
            handler: Box::new(LiteralHandler {
                operation: self.clone(),
                head: head.clone(),
                identity: format!("literal response edit {id}@{revision}"),
                report: pending(),
            }),
        })
    }
    fn apply(
        &self,
        head: &ResponseHead,
        input: &[u8],
    ) -> (BodyEditReport, Result<Vec<u8>, &'static str>) {
        let mut report = pending();
        report.before_bytes = Some(input.len());
        report.before_sha256 = Some(hash(input));
        let fail = |mut report: BodyEditReport, outcome: &str, message| {
            report.outcome = outcome.into();
            (report, Err(message))
        };
        if input.len() > self.max_bytes {
            return fail(
                report,
                "input-limit",
                "decoded input exceeds the selected bound",
            );
        }
        let types = head.headers.values("content-type").collect::<Vec<_>>();
        let utf8 = types.len() == 1 && std::str::from_utf8(types[0]).is_ok_and(valid_utf8_type);
        if !utf8 {
            return fail(
                report,
                "invalid-encoding",
                "supply one Content-Type with UTF-8 or no charset; other encodings are unsupported",
            );
        }
        if head.status == 206 || head.headers.values("content-range").next().is_some() {
            return fail(
                report,
                "partial-response",
                "partial responses cannot be edited as complete text",
            );
        }
        let Ok(text) = std::str::from_utf8(input) else {
            return fail(
                report,
                "invalid-text",
                "complete response is not valid UTF-8",
            );
        };
        let count = text.matches(&self.find).count();
        report.matches = Some(count);
        if self.expected_matches.is_some_and(|n| n != count) {
            return fail(
                report,
                "count-mismatch",
                "required total literal match count was not met",
            );
        }
        let replacements = if self.occurrences == Occurrences::First {
            count.min(1)
        } else {
            count
        };
        let size = input
            .len()
            .checked_sub(replacements * self.find.len())
            .and_then(|n| {
                replacements
                    .checked_mul(self.replacement.len())
                    .and_then(|r| n.checked_add(r))
            });
        if size.is_none_or(|size| size > self.max_bytes) {
            return fail(
                report,
                "output-limit",
                "replacement would exceed the selected output bound",
            );
        }
        let output = match self.occurrences {
            Occurrences::First => text.replacen(&self.find, &self.replacement, 1),
            Occurrences::All => text.replace(&self.find, &self.replacement),
        }
        .into_bytes();
        report.outcome = if count == 0 {
            "not-found"
        } else if output == input {
            "unchanged"
        } else {
            "applied"
        }
        .into();
        report.replacements = replacements;
        report.after_bytes = Some(output.len());
        report.after_sha256 = Some(hash(&output));
        (report, Ok(output))
    }
}
fn valid_utf8_type(value: &str) -> bool {
    let mut charset = None;
    for parameter in value.split(';').skip(1).map(str::trim) {
        let (name, value) = parameter.split_once('=').unwrap_or((parameter, ""));
        if name.trim().eq_ignore_ascii_case("charset") {
            if charset.is_some() {
                return false;
            }
            let value = value.trim();
            let value = if let Some(quoted) = value.strip_prefix('"') {
                let Some(end) = quoted.strip_suffix('"') else {
                    return false;
                };
                end
            } else {
                value
            };
            if !value.eq_ignore_ascii_case("utf-8") {
                return false;
            }
            charset = Some(());
        }
    }
    true
}
fn valid_media(media: &str) -> bool {
    media.len() <= 128
        && (media.starts_with("text/")
            || [
                "application/json",
                "application/javascript",
                "application/xml",
                "image/svg+xml",
            ]
            .contains(&media))
        && media
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-+.".contains(&b))
        && media.contains('/')
        && !media.ends_with('/')
}
struct LiteralHandler {
    operation: LiteralTransform,
    head: ResponseHead,
    identity: String,
    report: BodyEditReport,
}
impl BufferedBodyHandler for LiteralHandler {
    fn report(&self) -> Option<BodyEditReport> {
        Some(self.report.clone())
    }
    fn on_body(
        &mut self,
        body: BufferedBody,
    ) -> BoxBodyFuture<'_, Result<BufferedBody, BodyHookError>> {
        let (report, result) = self.operation.apply(&self.head, body.data());
        self.report = report;
        let result = result
            .map_err(|message| BodyHookError::Failed(format!("{}: {message}", self.identity)))
            .and_then(|bytes| {
                BufferedBody::try_new(
                    self.operation.max_bytes,
                    Bytes::from(bytes),
                    body.trailers().cloned(),
                )
                .map_err(BodyHookError::from)
            });
        Box::pin(async move { result })
    }
}
/// Tests a disabled or enabled draft without executing other rules or sending traffic.
/// # Errors
/// Rejects invalid rules and samples; operation failures remain explicit test outcomes.
pub fn test_text_transform(
    rule: &Rule,
    request: &RequestHead,
    response: &ResponseHead,
    input: &[u8],
) -> Result<TextTransformTest, String> {
    if input.len() > 1024 * 1024 {
        return Err("Sample exceeds 1 MiB".into());
    }
    crate::compile(vec![rule.clone()], crate::AutomationLimits::default())
        .map_err(|e| e.to_string())?;
    let operation = rule
        .response
        .text_transform
        .as_ref()
        .filter(|_| is_text_transform_rule(rule))
        .ok_or("Select a standalone literal response rule")?;
    let mut test = test_matcher(&rule.matcher, request)?;
    let regexes = compile_predicates(&rule.matcher.response_headers);
    let matched = rule.matcher.matches_response(
        request,
        response,
        &compile_predicates(&rule.matcher.request_headers),
        &regexes,
        rule.matcher
            .url
            .as_ref()
            .map(crate::UrlCondition::compile)
            .transpose()?
            .as_ref(),
    );
    test.checks.push(MatchCheck {
        label: "Response conditions".into(),
        matched,
        detail: "Status, status class and all response header conditions".into(),
    });
    test.checks.push(MatchCheck {
        label: "Response media type".into(),
        matched: operation.media_matches(&response.headers),
        detail: operation.media_type.clone(),
    });
    test.matched = test.checks.iter().all(|c| c.matched);
    let before_text = std::str::from_utf8(input)
        .map_err(|_| "Sample is not valid UTF-8")?
        .to_owned();
    let (report, after_text) = if test.matched {
        let (report, result) = operation.apply(response, input);
        (
            Some(report),
            result
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_else(|| before_text.clone()),
        )
    } else {
        (None, before_text.clone())
    };
    Ok(TextTransformTest {
        test,
        report,
        before_text,
        after_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RequestActions, ResponseActions, RuleMatcher};
    use transmog_core::{HeaderField, HttpLegVersion};
    fn operation() -> LiteralTransform {
        LiteralTransform {
            find: "old".into(),
            replacement: "new-long".into(),
            occurrences: Occurrences::All,
            expected_matches: None,
            media_type: "text/plain".into(),
            encoding: "utf-8".into(),
            max_bytes: 1024,
        }
    }
    fn head() -> ResponseHead {
        ResponseHead {
            status: 200,
            headers: HeaderBlock::from_fields(vec![
                HeaderField::try_new("Content-Type", "text/plain; charset=\"UTF-8\"").unwrap(),
            ]),
            source_version: HttpLegVersion::Http1,
        }
    }
    fn rule(operation: LiteralTransform) -> Rule {
        Rule {
            id: "literal".into(),
            display_name: None,
            enabled: false,
            revision: 1,
            priority: 0,
            matcher: RuleMatcher::default(),
            request: RequestActions::default(),
            response: ResponseActions {
                text_transform: Some(operation),
                ..Default::default()
            },
        }
    }
    #[test]
    fn literal_first_all_unicode_and_unrelated_bytes_are_exact() {
        let mut op = operation();
        op.expected_matches = Some(2);
        let input = "\u{feff}old\r\ntoken=é old untouched\n".as_bytes();
        let (all, result) = op.apply(&head(), input);
        assert_eq!(
            result.unwrap(),
            "\u{feff}new-long\r\ntoken=é new-long untouched\n".as_bytes()
        );
        assert_eq!(all.matches, Some(2));
        assert_eq!(all.replacements, 2);
        assert_ne!(all.before_sha256, all.after_sha256);
        op.occurrences = Occurrences::First;
        let (first, result) = op.apply(&head(), input);
        assert_eq!(first.matches, Some(2));
        assert_eq!(first.replacements, 1);
        assert_eq!(
            result.unwrap(),
            "\u{feff}new-long\r\ntoken=é old untouched\n".as_bytes()
        );
        op.find = ".*".into();
        assert_eq!(op.apply(&head(), b"old .*").0.matches, Some(1));
    }
    #[test]
    fn no_match_unchanged_and_required_count_have_distinct_evidence() {
        let mut op = operation();
        let (report, result) = op.apply(&head(), b"fresh token");
        assert_eq!(report.outcome, "not-found");
        assert_eq!(report.before_sha256, report.after_sha256);
        assert_eq!(result.unwrap(), b"fresh token");
        op.replacement = "old".into();
        assert_eq!(op.apply(&head(), b"old").0.outcome, "unchanged");
        op.expected_matches = Some(2);
        let (report, result) = op.apply(&head(), b"old");
        assert_eq!(report.outcome, "count-mismatch");
        assert_eq!(report.replacements, 0);
        assert!(result.is_err());
        assert!(report.after_sha256.is_none());
        op.expected_matches = Some(0);
        assert!(op.apply(&head(), b"fresh").1.is_ok());
    }
    #[test]
    fn invalid_partial_ambiguous_and_excessive_inputs_fail_without_partial_output() {
        let mut op = operation();
        assert_eq!(op.apply(&head(), &[255]).0.outcome, "invalid-text");
        let mut h = head();
        h.headers.replace_all(
            HeaderField::try_new("Content-Type", "text/plain; charset=iso-8859-1").unwrap(),
        );
        assert_eq!(op.apply(&h, b"old").0.outcome, "invalid-encoding");
        for media in [
            "text/plain; charset",
            "text/plain; charset=",
            "text/plain; charset=\"utf-8",
            "text/plain; charset=utf-8; charset=utf-8",
        ] {
            h = head();
            h.headers
                .replace_all(HeaderField::try_new("Content-Type", media).unwrap());
            assert_eq!(op.apply(&h, b"old").0.outcome, "invalid-encoding");
        }
        h = head();
        h.headers
            .push(HeaderField::try_new("Content-Type", "text/plain").unwrap());
        assert_eq!(op.apply(&h, b"old").0.outcome, "invalid-encoding");
        h = head();
        h.status = 206;
        assert_eq!(op.apply(&h, b"old").0.outcome, "partial-response");
        op.max_bytes = 4;
        assert_eq!(op.apply(&head(), b"old").0.outcome, "output-limit");
        assert_eq!(op.apply(&head(), b"12345").0.outcome, "input-limit");
    }
    #[test]
    fn draft_conditions_and_compile_guards_use_the_live_contract() {
        let request = crate::request_for_test("GET", "https://example.test/", &[]).unwrap();
        let mut r = rule(operation());
        r.matcher.response_status = Some(201);
        let test = test_text_transform(&r, &request, &head(), b"old").unwrap();
        assert!(!test.test.matched);
        assert!(test.report.is_none());
        assert_eq!(test.after_text, "old");
        r.matcher.response_status = Some(200);
        let test = test_text_transform(&r, &request, &head(), b"old").unwrap();
        assert_eq!(test.after_text, "new-long");
        assert_eq!(test.report.unwrap().outcome, "applied");
        for change in [0, 1, 2, 3] {
            let mut invalid = operation();
            match change {
                0 => invalid.find.clear(),
                1 => invalid.max_bytes = 1024 * 1024 + 1,
                2 => invalid.encoding = "utf-16".into(),
                _ => invalid.media_type = "image/png".into(),
            }
            assert!(
                crate::compile(vec![rule(invalid)], crate::AutomationLimits::default()).is_err()
            );
        }
        let mut r = rule(operation());
        r.response.discard_body = true;
        assert!(crate::compile(vec![r], crate::AutomationLimits::default()).is_err());
    }
}
