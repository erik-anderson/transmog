//! Bounded, transport-neutral HTTP content processing.
//!
//! This crate sits between canonical transfer-decoded body frames and semantic
//! body consumers. Codec engines are added behind these types; transport
//! adapters and product layers do not own content-coding policy.

#![deny(missing_docs)]

mod budget;
mod codec;
mod coding;
mod pipeline;
mod plan;
mod policy;

pub use budget::{ContentBudget, ContentLimitError, ContentLimits};
pub use codec::{
    ContentCodecError, ContentDecoder, ContentDecoderOptions, ContentEncoder, DeflateCompatibility,
};
pub use coding::{ContentCoding, ContentCodingError, ContentCodingStack};
pub use pipeline::{ContentBodyPipeline, ContentPipelineError};
pub use plan::{ContentLength, ContentOutput, ContentPlan};
pub use policy::{ContentMode, ContentPolicy};
