//! Conversion engine for moving worlds between Minecraft Bedrock and Java Edition.
//!
//! The engine is edition-agnostic at the edges: both sides decode into the shared
//! `bedrock_nbt::NbtTag` tree, so entity, item and block-entity translation is
//! plain in-memory tag rewriting. Bedrock storage access goes through
//! `bedrock-world`/`bedrock-leveldb` (the same crates the BMCBL host ships);
//! Java storage is handled by [`je::anvil`].

pub mod be;
pub mod compare;
pub mod convert;
pub mod error;
pub mod je;
pub mod mapping;
pub mod nbt;
pub mod report;

pub use convert::{ConversionOptions, ConversionPlan, Edition, Features};
pub use error::{Result, WorldPortError};
pub use report::{ConversionReport, Warning, WarningCode};
