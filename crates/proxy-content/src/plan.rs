use transmog_core::{HeaderBlock, HeaderField};

use crate::{ContentCodingError, ContentCodingStack, ContentLimits};

const INVALIDATED_FIELDS: &[&str] = &[
    "content-length",
    "content-md5",
    "digest",
    "content-digest",
    "repr-digest",
    "etag",
    "accept-ranges",
    "content-range",
];

/// Output representation coding after decoded-content hooks complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentOutput {
    /// Emit identity bytes and omit `Content-Encoding`.
    Identity,
    /// Reapply the original coding stack in its declared order.
    PreserveOriginal,
}

/// Whether final output length is known before its head is committed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentLength {
    /// Output is streamed and has no precomputed length.
    Streaming,
    /// Output was bounded and completely encoded before head commitment.
    Known(u64),
}

/// Immutable content-processing decision derived from validated headers.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use = "a content plan must be applied or explicitly rejected"]
pub struct ContentPlan {
    original: ContentCodingStack,
    output: ContentOutput,
    transforms_representation: bool,
}

impl ContentPlan {
    /// Builds a plan from all `Content-Encoding` fields.
    ///
    /// Callers opt into decoded processing by constructing this plan. The
    /// pass-through fast path should avoid plan application entirely.
    ///
    /// # Errors
    ///
    /// Returns [`ContentCodingError`] when the coding stack is malformed,
    /// unsupported, ambiguous, or too deep.
    pub fn from_headers(
        headers: &HeaderBlock,
        output: ContentOutput,
        limits: ContentLimits,
    ) -> Result<Self, ContentCodingError> {
        Self::for_body_pipeline(headers, output, limits, false)
    }

    /// Builds a plan for a body pipeline that may change identity bytes.
    ///
    /// `body_modified` must be true whenever a selected body stage can emit
    /// bytes different from its input, including replacement and discard.
    /// This invalidates representation metadata even when the source was
    /// already identity encoded.
    ///
    /// # Errors
    ///
    /// Returns [`ContentCodingError`] when the coding stack is malformed,
    /// unsupported, ambiguous, or too deep.
    pub fn for_body_pipeline(
        headers: &HeaderBlock,
        output: ContentOutput,
        limits: ContentLimits,
        body_modified: bool,
    ) -> Result<Self, ContentCodingError> {
        let original = ContentCodingStack::from_headers(headers, limits.max_coding_layers())?;
        let transforms_representation = body_modified || !original.is_empty();
        Ok(Self {
            original,
            output,
            transforms_representation,
        })
    }

    /// Original coding stack in sender application order.
    pub const fn original(&self) -> &ContentCodingStack {
        &self.original
    }

    /// Selected output representation coding.
    pub const fn output(&self) -> ContentOutput {
        self.output
    }

    /// Whether applying this plan changes representation bytes.
    pub fn transforms_representation(&self) -> bool {
        self.transforms_representation
    }

    /// Produces the identity-coded head shown to decoded-content hooks.
    pub fn headers_for_decoded_hooks(&self, source: &HeaderBlock) -> HeaderBlock {
        if !self.transforms_representation() {
            return source.clone();
        }
        repaired_headers(source, None, ContentLength::Streaming)
    }

    /// Removes stale representation validators from decoded trailers.
    ///
    /// Trailer ordering and unrelated fields are preserved. The same helper is
    /// used before hooks and before final output because digest values for the
    /// original encoded octets cannot describe decoded or recompressed bytes.
    pub fn repaired_trailers(&self, source: &HeaderBlock) -> HeaderBlock {
        if !self.transforms_representation() {
            return source.clone();
        }
        repaired_headers(source, None, ContentLength::Streaming)
    }

    /// Repairs a hook-produced identity head for final output.
    ///
    /// Validators and byte-range metadata remain invalidated even when the
    /// original coding is restored because recompression can change octets.
    pub fn headers_for_output(
        &self,
        decoded_head: &HeaderBlock,
        length: ContentLength,
    ) -> HeaderBlock {
        if !self.transforms_representation() {
            return decoded_head.clone();
        }
        let coding = match self.output {
            ContentOutput::Identity => None,
            ContentOutput::PreserveOriginal => self.original.header_value(),
        };
        repaired_headers(decoded_head, coding.as_deref(), length)
    }
}

fn repaired_headers(
    source: &HeaderBlock,
    content_encoding: Option<&str>,
    length: ContentLength,
) -> HeaderBlock {
    let mut output = source.clone();
    output.remove_all("content-encoding");
    for field in INVALIDATED_FIELDS {
        output.remove_all(field);
    }
    if let Some(content_encoding) = content_encoding {
        output.push(
            HeaderField::try_new("content-encoding", content_encoding)
                .expect("canonical content-coding tokens form a valid header"),
        );
    }
    if let ContentLength::Known(length) = length {
        output.push(
            HeaderField::try_new("content-length", length.to_string())
                .expect("a decimal u64 forms a valid header"),
        );
    }
    output
}

pub(crate) fn repaired_raw_output_headers(source: &HeaderBlock) -> HeaderBlock {
    let mut output = source.clone();
    for field in INVALIDATED_FIELDS {
        output.remove_all(field);
    }
    output
}

#[cfg(test)]
mod tests {
    use transmog_core::HeaderField;

    use super::*;

    fn encoded_headers() -> HeaderBlock {
        HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-type", "text/plain").unwrap(),
            HeaderField::try_new("content-encoding", "gzip, br").unwrap(),
            HeaderField::try_new("content-length", "42").unwrap(),
            HeaderField::try_new("etag", "\"encoded\"").unwrap(),
            HeaderField::try_new("content-md5", "obsolete").unwrap(),
            HeaderField::try_new("digest", "sha-256=old").unwrap(),
            HeaderField::try_new("content-digest", "sha-256=:old:").unwrap(),
            HeaderField::try_new("repr-digest", "sha-256=:old:").unwrap(),
            HeaderField::try_new("accept-ranges", "bytes").unwrap(),
            HeaderField::try_new("content-range", "bytes 0-41/42").unwrap(),
            HeaderField::try_new("x-preserved", "yes").unwrap(),
        ])
    }

    fn names(headers: &HeaderBlock) -> Vec<String> {
        headers
            .iter()
            .map(|field| String::from_utf8_lossy(field.name()).to_ascii_lowercase())
            .collect()
    }

    #[test]
    fn decoded_hook_head_removes_stale_representation_metadata() {
        let plan = ContentPlan::from_headers(
            &encoded_headers(),
            ContentOutput::PreserveOriginal,
            ContentLimits::default(),
        )
        .unwrap();
        let decoded = plan.headers_for_decoded_hooks(&encoded_headers());
        assert_eq!(names(&decoded), ["content-type", "x-preserved"]);
    }

    #[test]
    fn output_head_restores_only_selected_coding_and_known_length() {
        let source = encoded_headers();
        let preserve = ContentPlan::from_headers(
            &source,
            ContentOutput::PreserveOriginal,
            ContentLimits::default(),
        )
        .unwrap();
        let decoded = preserve.headers_for_decoded_hooks(&source);
        let output = preserve.headers_for_output(&decoded, ContentLength::Known(17));
        assert_eq!(
            output.values("content-encoding").collect::<Vec<_>>(),
            [b"gzip, br".as_slice()]
        );
        assert_eq!(
            output.values("content-length").collect::<Vec<_>>(),
            [b"17".as_slice()]
        );
        assert!(output.values("etag").next().is_none());

        let identity =
            ContentPlan::from_headers(&source, ContentOutput::Identity, ContentLimits::default())
                .unwrap();
        let output = identity.headers_for_output(&decoded, ContentLength::Streaming);
        assert!(output.values("content-encoding").next().is_none());
        assert!(output.values("content-length").next().is_none());
    }

    #[test]
    fn identity_plan_leaves_headers_byte_for_byte_unchanged() {
        let source = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-length", "3").unwrap(),
            HeaderField::try_new("etag", "\"same\"").unwrap(),
        ]);
        let plan =
            ContentPlan::from_headers(&source, ContentOutput::Identity, ContentLimits::default())
                .unwrap();
        assert!(!plan.transforms_representation());
        assert_eq!(plan.headers_for_decoded_hooks(&source), source);
        assert_eq!(
            plan.headers_for_output(&source, ContentLength::Streaming),
            source
        );
    }

    #[test]
    fn identity_body_modification_still_invalidates_representation_metadata() {
        let source = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-length", "3").unwrap(),
            HeaderField::try_new("etag", "\"identity\"").unwrap(),
            HeaderField::try_new("x-preserved", "yes").unwrap(),
        ]);
        let plan = ContentPlan::for_body_pipeline(
            &source,
            ContentOutput::Identity,
            ContentLimits::default(),
            true,
        )
        .unwrap();
        assert!(plan.transforms_representation());
        let repaired = plan.headers_for_output(
            &plan.headers_for_decoded_hooks(&source),
            ContentLength::Streaming,
        );
        assert!(repaired.values("content-length").next().is_none());
        assert!(repaired.values("etag").next().is_none());
        assert_eq!(
            repaired.values("x-preserved").collect::<Vec<_>>(),
            [b"yes".as_slice()]
        );
    }

    #[test]
    fn transformed_trailers_drop_stale_digests_but_preserve_other_fields() {
        let plan = ContentPlan::from_headers(
            &encoded_headers(),
            ContentOutput::Identity,
            ContentLimits::default(),
        )
        .unwrap();
        let trailers = HeaderBlock::from_fields(vec![
            HeaderField::try_new("content-digest", "sha-256=:old:").unwrap(),
            HeaderField::try_new("x-checkpoint", "done").unwrap(),
        ]);
        let repaired = plan.repaired_trailers(&trailers);
        assert!(repaired.values("content-digest").next().is_none());
        assert_eq!(
            repaired.values("x-checkpoint").collect::<Vec<_>>(),
            [b"done".as_slice()]
        );
    }
}
