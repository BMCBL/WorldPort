//! Bedrock Edition world access.
//!
//! Reading goes through `bedrock-world` (the same crate the BMCBL host uses), so
//! no Bedrock storage code is duplicated here.

pub mod inspect;
pub mod write;

pub use inspect::BedrockWorldSummary;
