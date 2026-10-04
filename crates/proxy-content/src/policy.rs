use crate::{ContentDecoderOptions, ContentLimits, DeflateCompatibility};

/// Runtime content-processing behavior for one proxy listener.
///
/// The policy is immutable and cheap to copy so an embedding application can
/// select behavior while construction is still single-threaded, then share it
/// across all exchanges handled by the listener.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ContentPolicy {
    mode: ContentMode,
    limits: ContentLimits,
    decoder_options: ContentDecoderOptions,
}

impl ContentPolicy {
    /// Disables decoding of non-identity representations.
    ///
    /// Neutral and raw hooks still run. Decoded hooks run for identity bodies;
    /// optional decoded hooks are declined for coded bodies, while required
    /// decoded hooks fail before body bytes are processed.
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Decodes bodies for semantic hooks and emits identity-coded output.
    pub fn inspect_to_identity(limits: ContentLimits) -> Self {
        Self {
            mode: ContentMode::InspectToIdentity,
            limits,
            decoder_options: ContentDecoderOptions::default(),
        }
    }

    /// Decodes bodies for semantic hooks and restores the original coding stack.
    pub fn preserve_original_output(limits: ContentLimits) -> Self {
        Self {
            mode: ContentMode::PreserveOriginalOutput,
            limits,
            decoder_options: ContentDecoderOptions::default(),
        }
    }

    /// Selects the HTTP `deflate` interoperability behavior.
    #[must_use]
    pub const fn with_deflate_compatibility(mut self, compatibility: DeflateCompatibility) -> Self {
        self.decoder_options = self
            .decoder_options
            .with_deflate_compatibility(compatibility);
        self
    }

    /// Selected content-processing mode.
    pub const fn mode(self) -> ContentMode {
        self.mode
    }

    /// Finite per-body content-processing limits.
    pub const fn limits(self) -> ContentLimits {
        self.limits
    }

    /// Decoder interoperability options.
    pub const fn decoder_options(self) -> ContentDecoderOptions {
        self.decoder_options
    }
}

/// Whether and how a listener decodes representations for semantic hooks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContentMode {
    /// Do not decode non-identity representations.
    #[default]
    Disabled,
    /// Decode for hooks and emit identity-coded output.
    InspectToIdentity,
    /// Decode for hooks and restore the source coding stack on output.
    PreserveOriginalOutput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_to_strict_disabled_processing() {
        let policy = ContentPolicy::default();
        assert_eq!(policy.mode(), ContentMode::Disabled);
        assert_eq!(policy.limits(), ContentLimits::default());
        assert_eq!(
            policy.decoder_options().deflate_compatibility(),
            DeflateCompatibility::StrictZlib
        );
    }

    #[test]
    fn constructors_and_compatibility_are_explicit() {
        let limits = ContentLimits::default();
        assert_eq!(
            ContentPolicy::inspect_to_identity(limits).mode(),
            ContentMode::InspectToIdentity
        );
        let policy = ContentPolicy::preserve_original_output(limits)
            .with_deflate_compatibility(DeflateCompatibility::AllowRaw);
        assert_eq!(policy.mode(), ContentMode::PreserveOriginalOutput);
        assert_eq!(
            policy.decoder_options().deflate_compatibility(),
            DeflateCompatibility::AllowRaw
        );
    }
}
