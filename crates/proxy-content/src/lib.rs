//! Bounded, transport-neutral HTTP content processing.
//!
//! This crate sits between canonical transfer-decoded body frames and semantic
//! body consumers. Codec engines are added behind these types; transport
//! adapters and product layers do not own content-coding policy.

#![deny(missing_docs)]

mod budget;
mod codec;
mod coding;
mod plan;

pub use budget::{ContentBudget, ContentLimitError, ContentLimits};
pub use codec::{ContentCodecError, ContentDecoder, ContentEncoder};
pub use coding::{ContentCoding, ContentCodingError, ContentCodingStack};
pub use plan::{ContentLength, ContentOutput, ContentPlan};
