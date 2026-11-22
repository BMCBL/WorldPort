//! Bedrock world writing on top of `bedrock-world`.
//!
//! Records are accumulated and committed in batches; subchunks are encoded as
//! v9 paletted storages, biomes as a quart-resolution `Data3D` payload.

use std::path::{Path, PathBuf};

use bedrock_nbt::NbtTag;
use bedrock_world::block::{BlockState, BlockStateStorageVersion};
use bedrock_world::chunk::subchunk::{SubChunk, SubChunkFormat, SubChunkVersion};
use bedrock_world::chunk::palette::BlockPalette;
use bedrock_world::world::{CreateOptions, OpenOptions, World};
use bedrock_world::block::BlockEntity;
use bedrock_world::{ActorUid, ChunkKey, ChunkPos, ChunkRecordTag, Dimension, encode_data3d_quart};
use bytes::Bytes;
use indexmap::IndexMap;

use crate::error::{Result, WorldPortError, io_result};
use crate::mapping::MappedState;
use crate::mapping::schema::BlockCatalog;

/// Chunks committed per LevelDB transaction.
const COMMIT_CHUNKS: usize = 64;

/// Bedrock blocks per subchunk section.
pub const SECTION_BLOCKS: usize = 16 * 16 * 16;

/// Writable Bedrock world.
pub struct BedrockOutput {
    world: World,
    pending: Vec<(Bytes, Bytes)>,
    pending_actors: Vec<(ChunkPos, ActorUid, Bytes)>,
    pending_chunks: usize,
}

impl BedrockOutput {
    /// Creates a new world at `path`.
    pub fn create(path: &Path, options: CreateOptions) -> Result<Self> {
        if path.exists() {
            let empty = path
                .read_dir()
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if !empty {
                return Err(WorldPortError::invalid(format!(
                    "{} already exists and is not empty",
                    path.display()
                )));
            }
        }
        io_result(path, std::fs::create_dir_all(path))?;
        let world = World::create(path, options).map_err(|error| {
            WorldPortError::other(format!("create Bedrock world {}: {error}", path.display()))
        })?;
        Ok(Self {
            world,
            pending: Vec::new(),
            pending_actors: Vec::new(),
            pending_chunks: 0,
        })
    }

    /// Opens an existing writable world.
    pub fn open(path: &Path) -> Result<Self> {
        let options = OpenOptions {
            read_only: false,
            ..OpenOptions::default()
        };
        let world = World::open(path, options)?;
        Ok(Self {
            world,
            pending: Vec::new(),
            pending_actors: Vec::new(),
            pending_chunks: 0,
        })
    }

    /// World path.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.world.path()
    }

    /// Underlying world handle.
    #[must_use]
    pub const fn world(&self) -> &World {
        &self.world
    }

    /// Queues a raw record.
    pub fn put_raw(&mut self, key: Bytes, value: Bytes) {
        self.pending.push((key, value));
    }

    /// Queues one actor record (`actorprefix<uid>` + digest entry).
    pub fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, payload: NbtTag) {
        if let Ok(bytes) = bedrock_nbt::serialize_root(&payload) {
            self.pending_actors.push((pos, uid, Bytes::from(bytes)));
        }
    }

    /// Counts one converted column; commits when the batch is full.
    pub fn finish_chunk(&mut self) -> Result<()> {
        self.pending_chunks += 1;
        if self.pending_chunks >= COMMIT_CHUNKS {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes all queued records in one transaction.
    pub fn flush(&mut self) -> Result<()> {
        if self.pending.is_empty() && self.pending_actors.is_empty() {
            return Ok(());
        }
        let mut transaction = self.world.transaction();
        for (key, value) in self.pending.drain(..) {
            transaction.put_raw_key(key, value);
        }
        for (pos, uid, value) in self.pending_actors.drain(..) {
            transaction.put_actor(pos, uid, value)?;
        }
        transaction.commit()?;
        self.pending_chunks = 0;
        Ok(())
    }

    /// Writes the mixed-in `level.dat`.
    pub fn write_level_dat(&self, root: NbtTag) -> Result<()> {
        let document = bedrock_world::LevelDatDocument::new(10, root);
        self.world.write_level_dat(&document)?;
        Ok(())
    }

    /// Flushes, compacts the LevelDB and returns the world path.
    ///
    /// Without compaction the output keeps one small SST per write batch plus a
    /// memtable dump: the same 6,968-chunk world came out at 26 MiB versus
    /// 3.5 MiB after compaction (Chunker compacts too), and a Bedrock client
    /// has to merge those tables on first open.
    pub fn finish(mut self) -> Result<PathBuf> {
        self.flush()?;
        self.world.compact_storage()?;
        let path = self.world.path().to_path_buf();
        Ok(path)
    }
}

/// Storage version written into every palette entry (1.26.60.0).
#[must_use]
pub const fn target_block_state_version() -> i32 {
    BlockStateStorageVersion::from_components(1, 26, 60, 0).raw()
}

/// One chunk section ready for encoding.
pub struct SectionWrite {
    /// Subchunk index (`block_y >> 4`).
    pub y: i8,
    /// Primary storage indices (`x*256 + z*16 + y`).
    pub primary: Vec<u16>,
    /// Primary palette.
    pub primary_palette: Vec<BlockState>,
    /// Secondary (liquid) storage indices, if any block is waterlogged or fluid.
    pub liquid: Option<Vec<u16>>,
    /// Secondary palette.
    pub liquid_palette: Vec<BlockState>,
    /// Biome ids per 4x4x4 cell (`(y*4+z)*4+x`).
    pub biomes: [u32; 64],
}

/// Encodes one section as a v9 subchunk payload.
///
/// Takes the section by value: the encoder needs owned palettes and indices,
/// and cloning them here copied 8 KiB of indices plus the full palette for
/// every section of every chunk.
pub fn encode_section(section: SectionWrite) -> Result<Bytes> {
    let mut storages = Vec::with_capacity(2);
    let mut liquid_taken = false;
    if let Some(liquid) = section.liquid
        && !section.liquid_palette.is_empty()
        && liquid.iter().any(|index| *index != 0)
    {
        liquid_taken = true;
        storages.push(BlockPalette::with_unpacked_indices(
            section.liquid_palette,
            liquid,
            None,
        ));
    }
    let _ = liquid_taken;
    storages.insert(
        0,
        BlockPalette::with_unpacked_indices(section.primary_palette, section.primary, None),
    );
    let subchunk = SubChunk {
        y: section.y,
        format: SubChunkFormat::Paletted {
            version: 9,
            storages,
        },
    };
    subchunk
        .encode(SubChunkVersion::V9)
        .map_err(|error| WorldPortError::other(format!("encode subchunk {}: {error}", section.y)))
}

/// Builds the `Data3D` payload (heightmap + 24 quart biome storages).
pub fn encode_biomes(height_map: &[i16; 256], sections: &[[u32; 64]]) -> Result<Bytes> {
    let bytes = encode_data3d_quart(height_map, sections)
        .map_err(|error| WorldPortError::other(format!("encode Data3D: {error}")))?;
    Ok(Bytes::from(bytes))
}

/// Chunk record keys for one column.
#[must_use]
pub fn chunk_keys(pos: ChunkPos) -> ChunkKeys {
    ChunkKeys {
        subchunk: ChunkKey::subchunk(pos, 0),
        data3d: ChunkKey::new(pos, ChunkRecordTag::Data3D),
        version: ChunkKey::new(pos, ChunkRecordTag::Version),
        finalized: ChunkKey::new(pos, ChunkRecordTag::FinalizedState),
        pos,
    }
}

/// Pre-built record keys for a column.
pub struct ChunkKeys {
    /// Template whose Y is replaced per subchunk.
    subchunk: ChunkKey,
    /// Heightmap + biomes.
    pub data3d: ChunkKey,
    /// Chunk version record.
    pub version: ChunkKey,
    /// Finalized state record.
    pub finalized: ChunkKey,
    /// Column position.
    pub pos: ChunkPos,
}

impl ChunkKeys {
    /// Key for the subchunk with index `y`.
    #[must_use]
    pub const fn subchunk(&self, y: i8) -> ChunkKey {
        ChunkKey::subchunk(self.pos, y)
    }
}

/// Value written into the `Version` record.
pub const CHUNK_VERSION_VALUE: u8 = 9;
/// Value written into `FinalizedState` (2 = populated).
pub const FINALIZED_STATE_POPULATED: i32 = 2;

/// Converts a mapped state to a persisted [`BlockState`] using the target schema types.
#[must_use]
pub fn to_block_state(mapped: &MappedState, catalog: &BlockCatalog) -> BlockState {
    let schema = catalog.block(&mapped.name);
    let version = target_block_state_version();
    let mut states = std::collections::BTreeMap::new();
    for (name, value) in &mapped.properties {
        let tag = match schema.and_then(|schema| schema.property(name)) {
            Some(property) => match property.kind() {
                crate::mapping::PropertyKind::Bool => NbtTag::Byte(i8::from(value == "true" || value == "1")),
                crate::mapping::PropertyKind::Int => NbtTag::Int(
                    value.parse::<i32>().unwrap_or(0),
                ),
                crate::mapping::PropertyKind::String => NbtTag::String(value.clone()),
            },
            None => heuristic_tag(name, value),
        };
        states.insert(name.clone(), tag);
    }
    BlockState {
        name: mapped.name.clone(),
        states,
        version: Some(version),
    }
}

fn heuristic_tag(name: &str, value: &str) -> NbtTag {
    const BIT_PROPS: &[&str] = &[
        "open_bit",
        "powered_bit",
        "button_pressed_bit",
        "upside_down_bit",
        "in_wall_bit",
        "persistent_bit",
        "triggered_bit",
        "trigger_bit",
        "output_lit_bit",
        "output_subtract_bit",
        "rail_data_bit",
        "occupied_bit",
        "head_piece_bit",
        "door_hinge_bit",
        "upper_block_bit",
        "attached_bit",
        "update_bit",
        "toggle_bit",
        "brewing_stand_slot_a_bit",
        "brewing_stand_slot_b_bit",
        "brewing_stand_slot_c_bit",
        "allow_block_commands",
    ];
    if BIT_PROPS.contains(&name) || name.ends_with("_bit") {
        return NbtTag::Byte(i8::from(value == "true" || value == "1"));
    }
    if matches!(value, "true" | "false") {
        return NbtTag::Byte(i8::from(value == "true"));
    }
    if let Ok(number) = value.parse::<i32>() {
        return NbtTag::Int(number);
    }
    NbtTag::String(value.to_string())
}

/// Convenience: converts a Java dimension to the Bedrock dimension.
#[must_use]
pub const fn dimension_of(java: crate::je::JeDimension) -> Dimension {
    match java {
        crate::je::JeDimension::Overworld => Dimension::Overworld,
        crate::je::JeDimension::Nether => Dimension::Nether,
        crate::je::JeDimension::End => Dimension::End,
    }
}

/// Builds a BE block entity record tag from raw Java block entity NBT.
#[must_use]
pub fn block_entity(id: &str, x: i32, y: i32, z: i32, extra: IndexMap<String, NbtTag>) -> BlockEntity {
    let mut nbt = IndexMap::new();
    nbt.insert("id".to_string(), NbtTag::String(id.to_string()));
    nbt.insert("x".to_string(), NbtTag::Int(x));
    nbt.insert("y".to_string(), NbtTag::Int(y));
    nbt.insert("z".to_string(), NbtTag::Int(z));
    for (key, value) in extra {
        nbt.insert(key, value);
    }
    BlockEntity {
        id: Some(id.to_string()),
        position: Some([x, y, z]),
        is_movable: None,
        custom_name: None,
        items: Vec::new(),
        nbt: NbtTag::Compound(nbt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::chunk::subchunk::SubChunkDecodeMode;
    use std::collections::BTreeMap;

    fn air() -> BlockState {
        BlockState {
            name: "minecraft:air".to_string(),
            states: BTreeMap::new(),
            version: Some(target_block_state_version()),
        }
    }

    fn stone() -> BlockState {
        BlockState {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: Some(target_block_state_version()),
        }
    }

    #[test]
    fn section_round_trips_through_v9() {
        let mut indices = vec![0_u16; SECTION_BLOCKS];
        for index in 0..SECTION_BLOCKS / 2 {
            indices[index] = 1;
        }
        let section = SectionWrite {
            y: 3,
            primary: indices,
            primary_palette: vec![air(), stone()],
            liquid: None,
            liquid_palette: Vec::new(),
            biomes: [1; 64],
        };
        let bytes = encode_section(section).expect("encode");
        assert_eq!(bytes[0], 9);
        assert_eq!(bytes[2], 3);
        let decoded = SubChunk::read(3, bytes, SubChunkDecodeMode::FullIndices).expect("decode");
        let state = decoded.block_state_at(0, 0, 0).expect("block");
        assert_eq!(state.name, "minecraft:stone");
    }

    #[test]
    fn liquid_layer_is_written_when_present() {
        let primary = vec![0_u16; SECTION_BLOCKS];
        let mut liquid = vec![0_u16; SECTION_BLOCKS];
        liquid[0] = 1;
        let section = SectionWrite {
            y: 0,
            primary,
            primary_palette: vec![air()],
            liquid: Some(liquid),
            liquid_palette: vec![air(), stone()],
            biomes: [1; 64],
        };
        let bytes = encode_section(section).expect("encode");
        assert_eq!(bytes[1], 2, "two storage layers");
    }
}
