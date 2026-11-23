//! Java Edition world access: layout discovery, `level.dat`, Anvil regions,
//! chunk NBT, player data and maps.

pub mod anvil;
pub mod chunk;
pub mod nbt;
pub mod player;
pub mod world;

pub use world::{JavaWorld, RegionCache, WorldLayout};

/// Java's three vanilla dimensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum JeDimension {
    /// Overworld (`dimensions/minecraft/overworld` or the world root in the old layout).
    Overworld,
    /// Nether (`the_nether`, `DIM-1`).
    Nether,
    /// End (`the_end`, `DIM1`).
    End,
}

impl JeDimension {
    /// Resource-location path used by the 1.21+ layout.
    #[must_use]
    pub const fn folder_name(self) -> &'static str {
        match self {
            Self::Overworld => "overworld",
            Self::Nether => "the_nether",
            Self::End => "the_end",
        }
    }

    /// Legacy folder name used before the `dimensions/` layout.
    #[must_use]
    pub const fn legacy_folder_name(self) -> &'static str {
        match self {
            Self::Overworld => "",
            Self::Nether => "DIM-1",
            Self::End => "DIM1",
        }
    }
}

/// Chunk column coordinates in Java Edition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct JeChunkPos {
    /// Chunk X.
    pub x: i32,
    /// Chunk Z.
    pub z: i32,
}

impl JeChunkPos {
    /// Builds a chunk position.
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Region file coordinates containing this chunk.
    #[must_use]
    pub const fn region(self) -> (i32, i32) {
        (self.x >> 5, self.z >> 5)
    }

    /// Local position inside the region file, `0..32`.
    #[must_use]
    pub const fn local(self) -> (u8, u8) {
        ((self.x & 31) as u8, (self.z & 31) as u8)
    }

    /// Renders the position for logs and reports.
    #[must_use]
    pub fn label(self) -> String {
        format!("{},{}", self.x, self.z)
    }
}

/// Block coordinates in Java Edition.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct JeBlockPos {
    /// Block X.
    pub x: i32,
    /// Block Y.
    pub y: i32,
    /// Block Z.
    pub z: i32,
}

impl JeBlockPos {
    /// Builds a block position.
    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Chunk column containing this block.
    #[must_use]
    pub const fn chunk(self) -> JeChunkPos {
        JeChunkPos::new(self.x >> 4, self.z >> 4)
    }
}

/// Maximum decoded size of a single chunk or gzip NBT document.
const MAX_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;

fn read_bounded(
    mut input: impl std::io::Read,
    initial_capacity: usize,
) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};

    let mut output = Vec::with_capacity(initial_capacity.min(MAX_PAYLOAD_BYTES as usize));
    input
        .by_ref()
        .take(MAX_PAYLOAD_BYTES + 1)
        .read_to_end(&mut output)?;
    if output.len() as u64 > MAX_PAYLOAD_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "decoded payload exceeds 64 MiB",
        ));
    }
    Ok(output)
}
