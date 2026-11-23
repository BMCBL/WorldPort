//! Block state schemas for both editions.
//!
//! The Java schema comes from Mojang's datagen reports (blocked per version,
//! stored gzipped under `mappings/data/java/<version>/blocks.json`). The Bedrock
//! schema is the catalog embedded in `bedrock-world`, which is generated from
//! `bedrock-samples` for the supported Bedrock release.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::OnceLock;


use crate::error::{Result, WorldPortError};

/// Property value type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyKind {
    /// `true`/`false`.
    Bool,
    /// Integer range such as `0..=15`.
    Int,
    /// Enum-like string values.
    String,
}

/// One block property and the values it accepts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertySchema {
    kind: PropertyKind,
    values: Vec<String>,
}

impl PropertySchema {
    /// Builds a property schema.
    #[must_use]
    pub fn new(kind: PropertyKind, values: Vec<String>) -> Self {
        Self { kind, values }
    }

    /// Value type.
    #[must_use]
    pub const fn kind(&self) -> PropertyKind {
        self.kind
    }

    /// Accepted values as strings.
    #[must_use]
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// Whether a string value is accepted.
    #[must_use]
    pub fn accepts(&self, value: &str) -> bool {
        self.values.iter().any(|allowed| allowed == value)
    }
}

/// One block and its properties.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlockSchema {
    properties: BTreeMap<String, PropertySchema>,
}
impl BlockSchema {
    /// Properties by name.
    #[must_use]
    pub const fn properties(&self) -> &BTreeMap<String, PropertySchema> {
        &self.properties
    }

    /// One property.
    #[must_use]
    pub fn property(&self, name: &str) -> Option<&PropertySchema> {
        self.properties.get(name)
    }
}

/// All blocks of one edition at one version.
#[derive(Clone, Debug)]
pub struct BlockCatalog {
    blocks: BTreeMap<String, BlockSchema>,
}

impl BlockCatalog {
    /// Builds a catalog from an explicit block map (used by tests and generated tables).
    #[must_use]
    pub const fn from_blocks(blocks: BTreeMap<String, BlockSchema>) -> Self {
        Self { blocks }
    }

    /// Blocks by full identifier (`minecraft:stone`).
    #[must_use]
    pub const fn blocks(&self) -> &BTreeMap<String, BlockSchema> {
        &self.blocks
    }

    /// One block.
    #[must_use]
    pub fn block(&self, name: &str) -> Option<&BlockSchema> {
        let key = if name.starts_with("minecraft:") {
            name.to_string()
        } else {
            format!("minecraft:{name}")
        };
        self.blocks.get(&key)
    }

    /// Whether the catalog knows a block.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.block(name).is_some()
    }

    /// Number of blocks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the catalog is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Java Edition blocks for one version.
#[derive(Debug)]
pub struct JavaCatalog {
    version: String,
    data_version: i32,
    catalog: BlockCatalog,
}

/// Every bundled catalog, parsed once.
fn catalogs() -> &'static Vec<JavaCatalog> {
    static CATALOGS: OnceLock<Vec<JavaCatalog>> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        let mut catalogs = Vec::new();
        for (version, data_version, blob) in baked::JAVA_CATALOGS {
            match JavaCatalog::parse_baked(blob) {
                Ok(mut catalog) => {
                    catalog.version = (*version).to_string();
                    catalog.data_version = *data_version;
                    // Guard against incomplete source data: a catalog without the
                    // basic blocks would prune legitimate terrain.
                    if catalog.catalog.contains("minecraft:stone")
                        && catalog.catalog.contains("minecraft:dirt")
                    {
                        catalogs.push(catalog);
                    } else {
                        tracing::warn!(
                            %version,
                            "bundled Java block catalog is incomplete and was ignored"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(%version, %error, "bundled Java block catalog failed to load");
                }
            }
        }
        catalogs.sort_by_key(|catalog| catalog.data_version);
        catalogs
    })
}

mod baked {
    include!(concat!(env!("OUT_DIR"), "/java_catalogs.rs"));
}

impl JavaCatalog {
    /// Loads the bundled catalog for an exact version label such as `1.21.4`.
    pub fn bundled(version: &str) -> Result<&'static Self> {
        catalogs()
            .iter()
            .find(|catalog| catalog.version == version)
            .ok_or_else(|| {
                WorldPortError::unsupported(format!(
                    "no bundled Java block catalog for version {version}"
                ))
            })
    }

    /// Picks the closest bundled catalog for a DataVersion: an exact match when
    /// present, otherwise the newest catalog not newer than the target.
    pub fn for_data_version(data_version: i32) -> Result<&'static Self> {
        let catalogs = catalogs();
        let exact = catalogs
            .iter()
            .find(|catalog| catalog.data_version == data_version);
        if let Some(catalog) = exact {
            return Ok(catalog);
        }
        let at_or_below = catalogs
            .iter()
            .filter(|catalog| catalog.data_version <= data_version)
            .max_by_key(|catalog| catalog.data_version);
        at_or_below.or_else(|| catalogs.first()).ok_or_else(|| {
            WorldPortError::unsupported("no bundled Java block catalogs are available")
        })
    }

    /// Version labels bundled into this build, oldest first.
    #[must_use]
    pub fn bundled_versions() -> Vec<(&'static str, i32)> {
        catalogs()
            .iter()
            .map(|catalog| (catalog.version.as_str(), catalog.data_version))
            .collect()
    }

    /// Resolves a version label (`1.21.4`) or DataVersion string to a catalog.
    pub fn for_version_or_data_version(value: &str) -> Result<&'static Self> {
        let trimmed = value.trim();
        if let Some(catalog) = catalogs()
            .iter()
            .find(|catalog| catalog.version == trimmed)
        {
            return Ok(catalog);
        }
        if let Ok(data_version) = trimmed.parse::<i32>() {
            return Self::for_data_version(data_version);
        }
        Err(WorldPortError::unsupported(format!(
            "unknown Java version {trimmed}; bundled: {:?}",
            Self::bundled_versions()
        )))
    }

    /// Decodes the compile-time baked catalog (`build.rs` → LZ4 + length-prefixed binary).
    fn parse_baked(blob: &[u8]) -> Result<Self> {
        let raw = lz4_flex::block::decompress_size_prepended(blob)
            .map_err(|error| WorldPortError::other(format!("java catalog lz4: {error}")))?;
        if raw.len() < 8 || &raw[..4] != b"WPCB" {
            return Err(WorldPortError::other("java catalog has a bad magic header"));
        }
        let mut reader = BinReader::new(&raw[4..]);
        let block_count = reader.u32()?;
        let mut blocks = BTreeMap::new();
        for _ in 0..block_count {
            let name = reader.string()?;
            let property_count = reader.u16()?;
            let mut properties = BTreeMap::new();
            for _ in 0..property_count {
                let property = reader.string()?;
                let kind = match reader.u8()? {
                    0 => PropertyKind::Bool,
                    1 => PropertyKind::Int,
                    _ => PropertyKind::String,
                };
                let value_count = reader.u16()?;
                let mut values = Vec::with_capacity(usize::from(value_count));
                for _ in 0..value_count {
                    values.push(reader.string()?);
                }
                properties.insert(property, PropertySchema::new(kind, values));
            }
            blocks.insert(name, BlockSchema { properties });
        }
        Ok(Self {
            version: String::new(),
            data_version: 0,
            catalog: BlockCatalog { blocks },
        })
    }

    /// Version label.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Java `DataVersion` of this catalog.
    #[must_use]
    pub const fn data_version(&self) -> i32 {
        self.data_version
    }

    /// Block lookup.
    #[must_use]
    pub const fn catalog(&self) -> &BlockCatalog {
        &self.catalog
    }

    /// Clones the block map into an owned catalog.
    #[must_use]
    pub fn to_catalog(&self) -> BlockCatalog {
        self.catalog.clone()
    }
}

/// Bedrock release this converter writes.
///
/// Kept in step with `be::write::target_block_state_version`: the catalog chosen
/// for mapping has to describe the states the writer emits.
pub const TARGET_BEDROCK_VERSION: &str = "1.26.60.0";

/// The Bedrock catalog for the release this converter writes.
///
/// Sourced from Chunker's per-build registries rather than `bedrock-world`'s
/// embedded sample: the sample only lists states its reference world happened to
/// contain, so whole blocks were missing from it, and it cannot describe any
/// release other than the one it was captured from.
#[must_use]
pub fn bedrock_catalog() -> BlockCatalog {
    bedrock_catalog_for(TARGET_BEDROCK_VERSION)
        .or_else(|| {
            bedrock_versions()
                .last()
                .map(|(_, catalog)| catalog.clone())
        })
        .unwrap_or_else(|| BlockCatalog::from_blocks(BTreeMap::new()))

}

/// Baked Bedrock catalogs: one blob, one shared string pool.
///
/// Generated by `core/build.rs` from `mappings/data/bedrock/<version>/`, which
/// `scripts/gen_bedrock_catalogs.py` produces from Chunker's registries. The data
/// source is Chunker's `cli/data/bedrock/<version>/block_states.json`, extracted
/// from the game itself (HiveGamesOSS/Chunker, MIT).
mod baked_bedrock {
    pub(super) const BLOB: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/bedrock_catalogs.bin.lz4"));
}

static BEDROCK_BY_VERSION: std::sync::OnceLock<Vec<(String, BlockCatalog)>> =
    std::sync::OnceLock::new();

fn bedrock_versions() -> &'static [(String, BlockCatalog)] {
    BEDROCK_BY_VERSION.get_or_init(|| {
        let Ok(raw) = lz4_flex::block::decompress_size_prepended(baked_bedrock::BLOB) else {
            tracing::warn!("baked Bedrock catalogs are corrupt");
            return Vec::new();
        };
        parse_bedrock_blob(&raw).unwrap_or_else(|error| {
            tracing::warn!(%error, "baked Bedrock catalogs failed to parse");
            Vec::new()
        })
    })
}

/// Decodes the pooled blob written by `build.rs`.
fn parse_bedrock_blob(raw: &[u8]) -> Result<Vec<(String, BlockCatalog)>> {
    if raw.len() < 6 || &raw[..4] != b"WPCB" {
        return Err(WorldPortError::other("Bedrock catalog blob has a bad header"));
    }
    let mut reader = BinReader::new(&raw[4..]);
    let version = reader.u16()?;
    if version != 2 {
        return Err(WorldPortError::other(format!(
            "Bedrock catalog blob version {version} is not supported"
        )));
    }
    let symbol_count = reader.u32()?;
    let mut symbols: Vec<String> = Vec::with_capacity(usize::try_from(symbol_count).unwrap_or(0));
    for _ in 0..symbol_count {
        symbols.push(reader.string()?);
    }
    let symbol = |index: u32| -> Result<String> {
        symbols
            .get(usize::try_from(index).unwrap_or(usize::MAX))
            .cloned()
            .ok_or_else(|| WorldPortError::other("Bedrock catalog symbol index out of range"))
    };

    let catalog_count = reader.u32()?;
    let mut catalogs = Vec::with_capacity(usize::try_from(catalog_count).unwrap_or(0));
    for _ in 0..catalog_count {
        let label = symbol(reader.u32()?)?;
        let block_count = reader.u32()?;
        let mut blocks = BTreeMap::new();
        for _ in 0..block_count {
            let name = symbol(reader.u32()?)?;
            let property_count = reader.u16()?;
            let mut properties = BTreeMap::new();
            for _ in 0..property_count {
                let property = symbol(reader.u32()?)?;
                let kind = match reader.u8()? {
                    0 => PropertyKind::Bool,
                    1 => PropertyKind::Int,
                    _ => PropertyKind::String,
                };
                let value_count = reader.u16()?;
                let mut values = Vec::with_capacity(usize::from(value_count));
                for _ in 0..value_count {
                    values.push(symbol(reader.u32()?)?);
                }
                properties.insert(property, PropertySchema::new(kind, values));
            }
            blocks.insert(name, BlockSchema { properties });
        }
        catalogs.push((label, BlockCatalog { blocks }));
    }
    catalogs.sort_by(|left, right| compare_version(&left.0, &right.0));
    Ok(catalogs)
}

/// Version labels for every bundled Bedrock catalog, oldest first.
#[must_use]
pub fn bedrock_catalog_versions() -> Vec<&'static str> {
    bedrock_versions()
        .iter()
        .map(|(version, _)| version.as_str())
        .collect()
}

/// The bundled Bedrock catalog for an exact version label.
#[must_use]
pub fn bedrock_catalog_for(version: &str) -> Option<BlockCatalog> {
    bedrock_versions()
        .iter()
        .find(|(candidate, _)| candidate == version)
        .map(|(_, catalog)| catalog.clone())
}

/// The newest bundled Bedrock catalog not newer than `version`.
///
/// A world reports a build such as `1.20.15.01`; the nearest catalog at or below
/// it is the closest description of what that build could contain.
#[must_use]
pub fn bedrock_catalog_at_or_below(version: &str) -> Option<BlockCatalog> {
    bedrock_versions()
        .iter()
        .filter(|(candidate, _)| compare_version(candidate, version) != std::cmp::Ordering::Greater)
        .max_by(|left, right| compare_version(&left.0, &right.0))
        .map(|(_, catalog)| catalog.clone())
}

/// Compares dotted version labels component by component, numerically.
fn compare_version(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(left), Some(right)) => {
                let left = left.parse::<u32>().unwrap_or(0);
                let right = right.parse::<u32>().unwrap_or(0);
                match left.cmp(&right) {
                    std::cmp::Ordering::Equal => {}
                    other => return other,
                }
            }
        }
    }
}

/// Bedrock block-state storage version reported by the embedded catalog./// Bedrock block-state storage version reported by the embedded catalog.
#[must_use]
pub fn bedrock_format_version() -> u32 {
    // The same value the writer stamps into every block state, so the two cannot
    // disagree; it used to come from `bedrock-world`'s catalog, which no longer
    // supplies any block data here.
    u32::try_from(crate::be::write::target_block_state_version()).unwrap_or(0)
}

fn clone_error(error: &WorldPortError) -> WorldPortError {
    match error {
        WorldPortError::Unsupported(message) => WorldPortError::Unsupported(message.clone()),
        WorldPortError::Other(message) => WorldPortError::Other(message.clone()),
        WorldPortError::Nbt(message) => WorldPortError::Nbt(message.clone()),
        WorldPortError::InvalidWorld(message) => WorldPortError::InvalidWorld(message.clone()),
        WorldPortError::Cancelled => WorldPortError::Cancelled,
        WorldPortError::Io { path, source } => WorldPortError::Io {
            path: path.clone(),
            source: std::io::Error::new(source.kind(), source.to_string()),
        },
    }
}

/// Cursor over the baked length-prefixed binary format.
struct BinReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> BinReader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| WorldPortError::other("java catalog offset overflow"))?;
        let slice = self
            .data
            .get(self.offset..end)
            .ok_or_else(|| WorldPortError::other("java catalog is truncated"))?;
        self.offset = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2 bytes")))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }

    fn string(&mut self) -> Result<String> {
        let length = usize::from(self.u16()?);
        let bytes = self.take(length)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| WorldPortError::other(format!("java catalog utf8: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_java_catalog_has_common_blocks() {
        let catalog = JavaCatalog::bundled("26.2").expect("bundled catalog");
        let stone = catalog.catalog().block("stone").expect("stone");
        assert!(stone.property("axis").is_none());
        let log = catalog.catalog().block("oak_log").expect("oak_log");
        let axis = log.property("axis").expect("axis");
        assert_eq!(axis.kind(), PropertyKind::String);
        assert_eq!(
            axis.values().iter().map(String::as_str).collect::<Vec<_>>(),
            ["x", "y", "z"]
        );
    }

    #[test]
    fn bedrock_catalog_has_common_blocks() {
        let catalog = bedrock_catalog();
        assert!(catalog.contains("minecraft:stone"));
        assert!(catalog.contains("minecraft:oak_planks"));
        let candle = catalog.block("minecraft:candle").expect("candle");
        assert!(candle.property("lit").is_some());
    }
}
