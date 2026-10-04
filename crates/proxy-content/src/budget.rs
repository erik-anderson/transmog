use std::{num::NonZeroUsize, time::Duration};

use thiserror::Error;

/// Finite byte and expansion limits for one content-processing body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentLimits {
    max_encoded_bytes: NonZeroUsize,
    max_decoded_bytes: NonZeroUsize,
    max_output_bytes: NonZeroUsize,
    max_decoder_window_bytes: NonZeroUsize,
    max_expansion_ratio: NonZeroUsize,
    expansion_slack_bytes: usize,
    max_coding_layers: NonZeroUsize,
    work: ContentWorkLimits,
}

impl ContentLimits {
    /// Creates an explicit finite limit set.
    pub const fn new(
        max_encoded_bytes: NonZeroUsize,
        max_decoded_bytes: NonZeroUsize,
        max_output_bytes: NonZeroUsize,
        max_decoder_window_bytes: NonZeroUsize,
        max_expansion_ratio: NonZeroUsize,
        expansion_slack_bytes: usize,
        max_coding_layers: NonZeroUsize,
    ) -> Self {
        Self {
            max_encoded_bytes,
            max_decoded_bytes,
            max_output_bytes,
            max_decoder_window_bytes,
            max_expansion_ratio,
            expansion_slack_bytes,
            max_coding_layers,
            work: ContentWorkLimits::DEFAULT,
        }
    }

    /// Replaces the cooperative codec work and deadline limits.
    #[must_use]
    pub const fn with_work_limits(mut self, work: ContentWorkLimits) -> Self {
        self.work = work;
        self
    }

    /// Maximum encoded input bytes.
    pub const fn max_encoded_bytes(self) -> NonZeroUsize {
        self.max_encoded_bytes
    }

    /// Maximum decoded bytes exposed to hooks.
    pub const fn max_decoded_bytes(self) -> NonZeroUsize {
        self.max_decoded_bytes
    }

    /// Maximum encoded or identity output bytes.
    pub const fn max_output_bytes(self) -> NonZeroUsize {
        self.max_output_bytes
    }

    /// Maximum history window a decoder may allocate for one coding layer.
    pub const fn max_decoder_window_bytes(self) -> NonZeroUsize {
        self.max_decoder_window_bytes
    }

    /// Maximum decoded-to-encoded expansion ratio after the slack allowance.
    pub const fn max_expansion_ratio(self) -> NonZeroUsize {
        self.max_expansion_ratio
    }

    /// Small-input decoded-byte allowance before ratio becomes the tighter cap.
    pub const fn expansion_slack_bytes(self) -> usize {
        self.expansion_slack_bytes
    }

    /// Maximum number of composed content-coding layers.
    pub const fn max_coding_layers(self) -> NonZeroUsize {
        self.max_coding_layers
    }

    /// Cooperative scheduling and active-work deadlines for each codec layer.
    pub const fn work_limits(self) -> ContentWorkLimits {
        self.work
    }
}

impl Default for ContentLimits {
    fn default() -> Self {
        Self::new(
            NonZeroUsize::new(16 * 1024 * 1024).expect("16 MiB is nonzero"),
            NonZeroUsize::new(64 * 1024 * 1024).expect("64 MiB is nonzero"),
            NonZeroUsize::new(64 * 1024 * 1024).expect("64 MiB is nonzero"),
            NonZeroUsize::new(16 * 1024 * 1024).expect("16 MiB is nonzero"),
            NonZeroUsize::new(100).expect("100 is nonzero"),
            64 * 1024,
            NonZeroUsize::new(4).expect("4 is nonzero"),
        )
    }
}

/// Finite cooperative scheduling and active-work deadlines for one codec layer.
///
/// The per-operation deadline covers one data-frame or completion call. The
/// per-body deadline accumulates time spent inside codec calls and cooperative
/// yields, but excludes time waiting for the next network body frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentWorkLimits {
    bytes_per_yield: NonZeroUsize,
    operation_duration: Duration,
    body_duration: Duration,
}

impl ContentWorkLimits {
    const DEFAULT: Self = Self {
        bytes_per_yield: NonZeroUsize::new(64 * 1024).expect("64 KiB is nonzero"),
        operation_duration: Duration::from_secs(2),
        body_duration: Duration::from_secs(30),
    };

    /// Creates explicit cooperative scheduling and deadline limits.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when either deadline is zero or the
    /// cumulative per-body deadline is shorter than one operation deadline.
    pub fn new(
        max_bytes_per_yield: NonZeroUsize,
        max_operation_duration: Duration,
        max_body_duration: Duration,
    ) -> Result<Self, ContentWorkLimitError> {
        if max_operation_duration.is_zero() {
            return Err(ContentWorkLimitError::ZeroOperationDuration);
        }
        if max_body_duration.is_zero() {
            return Err(ContentWorkLimitError::ZeroBodyDuration);
        }
        if max_body_duration < max_operation_duration {
            return Err(ContentWorkLimitError::BodyShorterThanOperation {
                operation: max_operation_duration,
                body: max_body_duration,
            });
        }
        Ok(Self {
            bytes_per_yield: max_bytes_per_yield,
            operation_duration: max_operation_duration,
            body_duration: max_body_duration,
        })
    }

    /// Maximum input or output byte progress between cooperative executor yields.
    pub const fn max_bytes_per_yield(self) -> NonZeroUsize {
        self.bytes_per_yield
    }

    /// Maximum active duration of one codec data-frame or completion call.
    pub const fn max_operation_duration(self) -> Duration {
        self.operation_duration
    }

    /// Maximum cumulative active duration of one codec layer for a body.
    pub const fn max_body_duration(self) -> Duration {
        self.body_duration
    }
}

impl Default for ContentWorkLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Invalid cooperative codec scheduling or deadline configuration.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ContentWorkLimitError {
    /// A codec operation deadline must permit some work.
    #[error("content codec operation deadline must be nonzero")]
    ZeroOperationDuration,
    /// A codec body deadline must permit some work.
    #[error("content codec body deadline must be nonzero")]
    ZeroBodyDuration,
    /// The cumulative deadline cannot be shorter than one operation deadline.
    #[error(
        "content codec body deadline {body:?} is shorter than operation deadline {operation:?}"
    )]
    BodyShorterThanOperation {
        /// Configured per-operation deadline.
        operation: Duration,
        /// Configured cumulative per-body deadline.
        body: Duration,
    },
}

/// Incremental resource accounting for one content-processing body.
#[derive(Clone, Debug)]
pub struct ContentBudget {
    limits: ContentLimits,
    encoded_bytes: usize,
    decoded_bytes: usize,
    output_bytes: usize,
}

impl ContentBudget {
    /// Starts empty accounting under `limits`.
    pub const fn new(limits: ContentLimits) -> Self {
        Self {
            limits,
            encoded_bytes: 0,
            decoded_bytes: 0,
            output_bytes: 0,
        }
    }

    /// Accepted encoded input bytes.
    pub const fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    /// Accepted decoded bytes.
    pub const fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }

    /// Accepted output bytes.
    pub const fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    /// Accounts encoded input without mutating state on rejection.
    ///
    /// # Errors
    ///
    /// Returns [`ContentLimitError::EncodedBytes`] if the new total exceeds
    /// the configured maximum or arithmetic range.
    pub fn record_encoded(&mut self, bytes: usize) -> Result<(), ContentLimitError> {
        let Some(attempted) = self.encoded_bytes.checked_add(bytes) else {
            return Err(ContentLimitError::EncodedBytes {
                limit: self.limits.max_encoded_bytes.get(),
                attempted: usize::MAX,
            });
        };
        if attempted > self.limits.max_encoded_bytes.get() {
            return Err(ContentLimitError::EncodedBytes {
                limit: self.limits.max_encoded_bytes.get(),
                attempted,
            });
        }
        self.encoded_bytes = attempted;
        Ok(())
    }

    /// Accounts decoded output and enforces both absolute and ratio limits.
    ///
    /// # Errors
    ///
    /// Returns [`ContentLimitError`] without mutating state if an absolute or
    /// expansion bound would be exceeded.
    pub fn record_decoded(&mut self, bytes: usize) -> Result<(), ContentLimitError> {
        let Some(attempted) = self.decoded_bytes.checked_add(bytes) else {
            return Err(ContentLimitError::DecodedBytes {
                limit: self.limits.max_decoded_bytes.get(),
                attempted: usize::MAX,
            });
        };
        if attempted > self.limits.max_decoded_bytes.get() {
            return Err(ContentLimitError::DecodedBytes {
                limit: self.limits.max_decoded_bytes.get(),
                attempted,
            });
        }
        let ratio_limit = self
            .encoded_bytes
            .saturating_mul(self.limits.max_expansion_ratio.get())
            .max(self.limits.expansion_slack_bytes);
        if attempted > ratio_limit {
            return Err(ContentLimitError::ExpansionRatio {
                encoded: self.encoded_bytes,
                decoded: attempted,
                ratio: self.limits.max_expansion_ratio.get(),
                slack: self.limits.expansion_slack_bytes,
            });
        }
        self.decoded_bytes = attempted;
        Ok(())
    }

    /// Accounts final identity or re-encoded output bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ContentLimitError::OutputBytes`] if the new total exceeds the
    /// configured maximum or arithmetic range.
    pub fn record_output(&mut self, bytes: usize) -> Result<(), ContentLimitError> {
        let Some(attempted) = self.output_bytes.checked_add(bytes) else {
            return Err(ContentLimitError::OutputBytes {
                limit: self.limits.max_output_bytes.get(),
                attempted: usize::MAX,
            });
        };
        if attempted > self.limits.max_output_bytes.get() {
            return Err(ContentLimitError::OutputBytes {
                limit: self.limits.max_output_bytes.get(),
                attempted,
            });
        }
        self.output_bytes = attempted;
        Ok(())
    }
}

/// A finite content-processing resource limit was exceeded.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ContentLimitError {
    /// Encoded input exceeded its byte bound.
    #[error("encoded content attempted {attempted} bytes; limit is {limit}")]
    EncodedBytes {
        /// Configured limit.
        limit: usize,
        /// Attempted total, saturated on arithmetic overflow.
        attempted: usize,
    },
    /// Decoded output exceeded its absolute byte bound.
    #[error("decoded content attempted {attempted} bytes; limit is {limit}")]
    DecodedBytes {
        /// Configured limit.
        limit: usize,
        /// Attempted total, saturated on arithmetic overflow.
        attempted: usize,
    },
    /// Decoded output exceeded the ratio/slack bound.
    #[error(
        "content expansion exceeded {ratio}:1 with {slack}-byte slack ({encoded} encoded, {decoded} decoded)"
    )]
    ExpansionRatio {
        /// Accepted encoded input bytes.
        encoded: usize,
        /// Attempted decoded bytes.
        decoded: usize,
        /// Configured expansion ratio.
        ratio: usize,
        /// Configured small-input allowance.
        slack: usize,
    },
    /// Final output exceeded its byte bound.
    #[error("content output attempted {attempted} bytes; limit is {limit}")]
    OutputBytes {
        /// Configured limit.
        limit: usize,
        /// Attempted total, saturated on arithmetic overflow.
        attempted: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ContentLimits {
        ContentLimits::new(
            NonZeroUsize::new(10).unwrap(),
            NonZeroUsize::new(30).unwrap(),
            NonZeroUsize::new(20).unwrap(),
            NonZeroUsize::new(8).unwrap(),
            NonZeroUsize::new(2).unwrap(),
            6,
            NonZeroUsize::new(2).unwrap(),
        )
    }

    #[test]
    fn exact_absolute_limits_are_accepted_and_rejection_does_not_mutate() {
        let mut budget = ContentBudget::new(limits());
        budget.record_encoded(10).unwrap();
        assert_eq!(budget.encoded_bytes(), 10);
        assert_eq!(
            budget.record_encoded(1),
            Err(ContentLimitError::EncodedBytes {
                limit: 10,
                attempted: 11
            })
        );
        assert_eq!(budget.encoded_bytes(), 10);

        budget.record_decoded(20).unwrap();
        budget.record_output(20).unwrap();
        assert_eq!(budget.decoded_bytes(), 20);
        assert_eq!(budget.output_bytes(), 20);
        assert!(matches!(
            budget.record_output(1),
            Err(ContentLimitError::OutputBytes { .. })
        ));
        assert_eq!(budget.output_bytes(), 20);
    }

    #[test]
    fn ratio_uses_small_input_slack_then_becomes_tighter() {
        let mut budget = ContentBudget::new(limits());
        budget.record_encoded(1).unwrap();
        budget.record_decoded(6).unwrap();
        assert_eq!(
            budget.record_decoded(1),
            Err(ContentLimitError::ExpansionRatio {
                encoded: 1,
                decoded: 7,
                ratio: 2,
                slack: 6
            })
        );

        let mut budget = ContentBudget::new(limits());
        budget.record_encoded(10).unwrap();
        budget.record_decoded(20).unwrap();
        assert!(matches!(
            budget.record_decoded(1),
            Err(ContentLimitError::ExpansionRatio { .. })
        ));
    }

    #[test]
    fn arithmetic_overflow_is_a_limit_error_without_mutation() {
        let enormous = ContentLimits::new(
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(usize::MAX).unwrap(),
            NonZeroUsize::new(1).unwrap(),
            usize::MAX,
            NonZeroUsize::new(1).unwrap(),
        );
        let mut budget = ContentBudget::new(enormous);
        budget.record_encoded(usize::MAX - 1).unwrap();
        assert_eq!(
            budget.record_encoded(2),
            Err(ContentLimitError::EncodedBytes {
                limit: usize::MAX,
                attempted: usize::MAX
            })
        );
        assert_eq!(budget.encoded_bytes(), usize::MAX - 1);
    }

    #[test]
    fn work_limits_are_finite_validated_and_replaceable() {
        let defaults = ContentWorkLimits::default();
        assert_eq!(defaults.max_bytes_per_yield().get(), 64 * 1024);
        assert_eq!(defaults.max_operation_duration(), Duration::from_secs(2));
        assert_eq!(defaults.max_body_duration(), Duration::from_secs(30));

        assert_eq!(
            ContentWorkLimits::new(
                NonZeroUsize::new(1).unwrap(),
                Duration::ZERO,
                Duration::from_secs(1)
            ),
            Err(ContentWorkLimitError::ZeroOperationDuration)
        );
        assert_eq!(
            ContentWorkLimits::new(
                NonZeroUsize::new(1).unwrap(),
                Duration::from_secs(1),
                Duration::ZERO
            ),
            Err(ContentWorkLimitError::ZeroBodyDuration)
        );
        assert!(matches!(
            ContentWorkLimits::new(
                NonZeroUsize::new(1).unwrap(),
                Duration::from_secs(2),
                Duration::from_secs(1)
            ),
            Err(ContentWorkLimitError::BodyShorterThanOperation { .. })
        ));

        let custom = ContentWorkLimits::new(
            NonZeroUsize::new(4 * 1024).unwrap(),
            Duration::from_millis(50),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(limits().with_work_limits(custom).work_limits(), custom);
    }
}
