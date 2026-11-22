//! Java point-of-interest storage and the shared interest model.
//!
//! Village interests exist in both editions with different shapes: Java keeps a
//! per-block record with a ticket budget, Bedrock keeps a per-villager claim
//! list. This module owns the Java side plus the block-derived fallback that
//! reconstructs interests from the terrain when no record survives.

pub mod bedrock;
pub mod blocks;
pub mod java;

pub use bedrock::{PoiConversion, convert, free_tickets, kind_of_java_type};
pub use blocks::{BlockInterest, InterestInventory, classify, report_inventory};
pub use java::{
    PoiChunk, PoiRecord, chunk_of, encode_chunk, group_by_section, read_chunk, read_region,
    section_of, write_region,
};
