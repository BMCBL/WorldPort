//! Java player data (`playerdata/<uuid>.dat`).

use std::path::Path;

use crate::error::Result;
use crate::je::nbt::{self as je_nbt, NamedTag};

/// Reads one player data file (gzip-compressed NBT).
pub fn read(path: &Path) -> Result<NamedTag> {
    je_nbt::read_gzip(path)
}

/// Writes one player data file.
pub fn write(path: &Path, player: &NamedTag) -> Result<()> {
    je_nbt::write_gzip(path, &player.name, &player.tag)
}
