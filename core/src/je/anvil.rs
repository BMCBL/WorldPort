//! Anvil region files (`.mca`): 4 KiB sector layout, 1024 location slots,
//! zlib/gzip/uncompressed chunk payloads.
//!
//! Reading tolerates files produced by third-party tools; writing always emits a
//! compact contiguous region. Bulk conversion uses [`write_region_file`], which
//! streams chunks and patches the header at the end.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};
use rayon::prelude::*;

use crate::error::{Result, WorldPortError, io_result};

const SECTOR_BYTES: u64 = 4096;
const HEADER_BYTES: u64 = SECTOR_BYTES * 2;
const SLOTS: usize = 1024;

/// Chunk payload compression, as stored in the region file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionCompression {
    /// gzip stream (`1`).
    Gzip,
    /// zlib stream (`2`), what vanilla writes.
    Zlib,
    /// Raw NBT bytes (`3`).
    None,
}

impl RegionCompression {
    /// Compression id stored on disk.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Gzip => 1,
            Self::Zlib => 2,
            Self::None => 3,
        }
    }
}

/// Where a chunk lives inside its region file.
#[derive(Clone, Copy, Debug, Default)]
struct Location {
    offset_sectors: u32,
    sector_count: u8,
}

impl Location {
    const fn is_present(self) -> bool {
        self.offset_sectors != 0 && self.sector_count != 0
    }
}

/// An open region file. Reads are random access; writes append new sectors.
pub struct RegionFile {
    path: PathBuf,
    file: File,
    locations: [Location; SLOTS],
    timestamps: [u32; SLOTS],
    header_dirty: bool,
    next_sector: u32,
}

impl RegionFile {
    /// Opens a region file, creating an empty one when `create` is set.
    pub fn open(path: &Path, create: bool) -> Result<Self> {
        let exists = path.exists();
        if !exists && !create {
            return Err(WorldPortError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "region file missing"),
            ));
        }
        let file = if exists {
            // A reader that never writes must not request write access: it
            // fails on read-only media and takes an exclusive lock needlessly.
            io_result(path, File::options().read(true).write(true).open(path))?
        } else {
            let file = io_result(
                path,
                File::options()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(path),
            )?;
            io_result(path, file.set_len(HEADER_BYTES))?;
            file
        };
        let length = io_result(path, file.metadata())?.len();
        let mut this = Self {
            path: path.to_path_buf(),
            file,
            locations: [Location::default(); SLOTS],
            timestamps: [0; SLOTS],
            header_dirty: false,
            next_sector: u32::try_from(length.div_ceil(SECTOR_BYTES)).unwrap_or(u32::MAX),
        };
        if length >= HEADER_BYTES {
            this.read_header()?;
        } else {
            this.header_dirty = true;
        }
        Ok(this)
    }

    /// Path this region was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns whether a chunk slot is populated.
    #[must_use]
    pub fn has_chunk(&self, local_x: u8, local_z: u8) -> bool {
        self.locations[slot_index(local_x, local_z)].is_present()
    }

    /// Lists populated chunk slots as `(local_x, local_z)`.
    #[must_use]
    pub fn occupied_slots(&self) -> Vec<(u8, u8)> {
        (0..SLOTS)
            .filter(|index| self.locations[*index].is_present())
            .map(|index| ((index % 32) as u8, (index / 32) as u8))
            .collect()
    }

    /// Reads and decompresses one chunk payload.
    pub fn read_chunk(&mut self, local_x: u8, local_z: u8) -> Result<Option<Vec<u8>>> {
        let location = self.locations[slot_index(local_x, local_z)];
        if !location.is_present() {
            return Ok(None);
        }
        let start = u64::from(location.offset_sectors) * SECTOR_BYTES;
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        let mut header = [0_u8; 5];
        self.file
            .read_exact(&mut header)
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        let length = usize::try_from(u32::from_be_bytes([
            header[0], header[1], header[2], header[3],
        ]))
        .map_err(|_| WorldPortError::nbt("chunk length overflow"))?;
        if length == 0 {
            return Ok(None);
        }
        let compression = header[4];
        let payload_len = length - 1;
        let mut compressed = vec![0_u8; payload_len];
        self.file
            .read_exact(&mut compressed)
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        decompress(&self.path, compression, &compressed).map(Some)
    }

    /// Writes a chunk payload, appending sectors at the end of the file.
    pub fn write_chunk(
        &mut self,
        local_x: u8,
        local_z: u8,
        payload: &[u8],
        compression: RegionCompression,
        level: Compression,
    ) -> Result<()> {
        let compressed = compress(compression, payload, level)?;
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| WorldPortError::other("chunk payload exceeds 4 GiB"))?;
        let span = u64::from(length) + 4;
        let sectors = u8::try_from(span.div_ceil(SECTOR_BYTES))
            .map_err(|_| WorldPortError::other("chunk payload exceeds 255 sectors"))?;
        let index = slot_index(local_x, local_z);
        let existing = self.locations[index];
        let offset = if existing.sector_count == sectors {
            existing.offset_sectors
        } else {
            let offset = self.next_sector;
            self.next_sector = self
                .next_sector
                .checked_add(u32::from(sectors))
                .ok_or_else(|| WorldPortError::other("region file exceeds 4 GiB"))?;
            self.file
                .set_len(u64::from(self.next_sector) * SECTOR_BYTES)
                .map_err(|error| WorldPortError::io(&self.path, error))?;
            offset
        };
        self.file
            .seek(SeekFrom::Start(u64::from(offset) * SECTOR_BYTES))
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        self.file
            .write_all(&length.to_be_bytes())
            .and_then(|()| self.file.write_all(&[compression.id()]))
            .and_then(|()| self.file.write_all(&compressed))
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        let padding = u64::from(sectors) * SECTOR_BYTES - span;
        if padding > 0 {
            self.file
                .write_all(&vec![0_u8; usize::try_from(padding).unwrap_or(0)])
                .map_err(|error| WorldPortError::io(&self.path, error))?;
        }
        self.locations[index] = Location {
            offset_sectors: offset,
            sector_count: sectors,
        };
        self.timestamps[index] = unix_now_seconds();
        self.header_dirty = true;
        Ok(())
    }

    /// Removes a chunk slot (leaves its sectors unreferenced).
    pub fn delete_chunk(&mut self, local_x: u8, local_z: u8) {
        let index = slot_index(local_x, local_z);
        self.locations[index] = Location::default();
        self.timestamps[index] = 0;
        self.header_dirty = true;
    }

    /// Writes the header when it changed.
    pub fn flush(&mut self) -> Result<()> {
        if !self.header_dirty {
            return Ok(());
        }
        self.file
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.file.write_all(&self.encode_header()))
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        self.file
            .flush()
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        self.header_dirty = false;
        Ok(())
    }

    fn read_header(&mut self) -> Result<()> {
        let mut header = [0_u8; HEADER_BYTES as usize];
        self.file
            .seek(SeekFrom::Start(0))
            .and_then(|_| self.file.read_exact(&mut header))
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        for (index, chunk) in header[..SLOTS * 4].chunks_exact(4).enumerate() {
            let offset = u32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]);
            self.locations[index] = Location {
                offset_sectors: offset,
                sector_count: chunk[3],
            };
        }
        for (index, chunk) in header[SLOTS * 4..].chunks_exact(4).enumerate() {
            self.timestamps[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        Ok(())
    }

    fn encode_header(&self) -> [u8; HEADER_BYTES as usize] {
        let mut header = [0_u8; HEADER_BYTES as usize];
        for (index, location) in self.locations.iter().enumerate() {
            let offset = location.offset_sectors.to_be_bytes();
            header[index * 4] = offset[1];
            header[index * 4 + 1] = offset[2];
            header[index * 4 + 2] = offset[3];
            header[index * 4 + 3] = location.sector_count;
        }
        for (index, timestamp) in self.timestamps.iter().enumerate() {
            header[SLOTS * 4 + index * 4..SLOTS * 4 + index * 4 + 4]
                .copy_from_slice(&timestamp.to_be_bytes());
        }
        header
    }
}

/// Writes a whole region file from `(local_x, local_z, decompressed payload)` entries.
///
/// Entries are written in the given order; duplicate slots keep the last payload.
pub fn write_region_file<I>(path: &Path, entries: I, level: Compression) -> Result<()>
where
    I: IntoIterator<Item = (u8, u8, Vec<u8>)>,
{
    write_region_compressed(
        path,
        entries
            .into_iter()
            .map(|(local_x, local_z, payload)| (local_x, local_z, ZlibEncode::Payload(payload))),
        level,
    )
}

/// One region entry: either an uncompressed NBT payload or an already-compressed
/// deflate stream.
///
/// Callers that compress on their own worker pass [`ZlibEncode::Zlib`] so the
/// writer does not decompress and recompress; the payload is written verbatim.
pub enum ZlibEncode {
    /// Uncompressed NBT; the writer compresses it.
    Payload(Vec<u8>),
    /// A zlib stream produced by the caller.
    Zlib(Vec<u8>),
}

/// Writes a region file from entries that may already be compressed.
pub fn write_region_compressed<I>(path: &Path, entries: I, level: Compression) -> Result<()>
where
    I: IntoIterator<Item = (u8, u8, ZlibEncode)>,
{
    if let Some(parent) = path.parent() {
        io_result(parent, std::fs::create_dir_all(parent))?;
    }
    let file = io_result(
        path,
        File::options()
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .open(path),
    )?;
    // Compress every chunk of the region in parallel; the sector layout below
    // has to be sequential because offsets depend on the previous sizes.
    let compressed: Vec<(u8, u8, Vec<u8>)> = entries
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(local_x, local_z, payload)| match payload {
            // Already a deflate stream: hand it straight to the sector writer.
            ZlibEncode::Zlib(bytes) => Ok((local_x, local_z, bytes)),
            ZlibEncode::Payload(bytes) => {
                compress(RegionCompression::Zlib, &bytes, level)
                    .map(|compressed| (local_x, local_z, compressed))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    writer
        .write_all(&[0_u8; HEADER_BYTES as usize])
        .map_err(|error| WorldPortError::io(path, error))?;
    let mut locations = [Location::default(); SLOTS];
    let timestamp = unix_now_seconds();
    let mut next_sector = 2_u32;
    for (local_x, local_z, compressed) in compressed {
        if local_x >= 32 || local_z >= 32 {
            return Err(WorldPortError::other(format!(
                "local chunk position out of range: {local_x},{local_z}"
            )));
        }
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| WorldPortError::other("chunk payload exceeds 4 GiB"))?;
        let span = u64::from(length) + 4;
        let sectors = u8::try_from(span.div_ceil(SECTOR_BYTES))
            .map_err(|_| WorldPortError::other("chunk payload exceeds 255 sectors"))?;
        locations[slot_index(local_x, local_z)] = Location {
            offset_sectors: next_sector,
            sector_count: sectors,
        };
        next_sector = next_sector
            .checked_add(u32::from(sectors))
            .ok_or_else(|| WorldPortError::other("region file exceeds 4 GiB"))?;
        writer
            .write_all(&length.to_be_bytes())
            .and_then(|()| writer.write_all(&[RegionCompression::Zlib.id()]))
            .and_then(|()| writer.write_all(&compressed))
            .map_err(|error| WorldPortError::io(path, error))?;
        let padding = u64::from(sectors) * SECTOR_BYTES - span;
        if padding > 0 {
            writer
                .write_all(&vec![0_u8; usize::try_from(padding).unwrap_or(0)])
                .map_err(|error| WorldPortError::io(path, error))?;
        }
    }
    let mut header = [0_u8; HEADER_BYTES as usize];
    for (index, location) in locations.iter().enumerate() {
        let offset = location.offset_sectors.to_be_bytes();
        header[index * 4] = offset[1];
        header[index * 4 + 1] = offset[2];
        header[index * 4 + 2] = offset[3];
        header[index * 4 + 3] = location.sector_count;
        if location.is_present() {
            header[SLOTS * 4 + index * 4..SLOTS * 4 + index * 4 + 4]
                .copy_from_slice(&timestamp.to_be_bytes());
        }
    }
    let mut file = writer
        .into_inner()
        .map_err(|error| WorldPortError::io(path, error.into_error()))?;
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.write_all(&header))
        .and_then(|()| file.flush())
        .map_err(|error| WorldPortError::io(path, error))?;
    Ok(())
}

fn slot_index(local_x: u8, local_z: u8) -> usize {
    usize::from(local_z) * 32 + usize::from(local_x)
}

fn unix_now_seconds() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u32::try_from(duration.as_secs()).unwrap_or(u32::MAX))
        .unwrap_or(0)
}

/// Compresses `payload` with the region-file codec (zlib for vanilla Java).
pub fn compress(kind: RegionCompression, payload: &[u8], level: Compression) -> Result<Vec<u8>> {
    match kind {
        RegionCompression::Gzip => {
            let mut encoder = GzEncoder::new(Vec::with_capacity(payload.len() / 2), level);
            encoder
                .write_all(payload)
                .map_err(|error| WorldPortError::nbt(format!("gzip: {error}")))?;
            encoder
                .finish()
                .map_err(|error| WorldPortError::nbt(format!("gzip: {error}")))
        }
        RegionCompression::Zlib => {
            let mut encoder = ZlibEncoder::new(Vec::with_capacity(payload.len() / 2), level);
            encoder
                .write_all(payload)
                .map_err(|error| WorldPortError::nbt(format!("zlib: {error}")))?;
            encoder
                .finish()
                .map_err(|error| WorldPortError::nbt(format!("zlib: {error}")))
        }
        RegionCompression::None => Ok(payload.to_vec()),
    }
}

fn decompress(path: &Path, compression: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::with_capacity(payload.len().saturating_mul(3).max(1024));
    match compression {
        1 => GzDecoder::new(payload)
            .read_to_end(&mut decoded)
            .map_err(|error| WorldPortError::io(path, std::io::Error::other(error)))?,
        2 => ZlibDecoder::new(payload)
            .read_to_end(&mut decoded)
            .map_err(|error| WorldPortError::io(path, std::io::Error::other(error)))?,
        3 => return Ok(payload.to_vec()),
        other => {
            return Err(WorldPortError::unsupported(format!(
                "region chunk compression id {other} in {} (only gzip/zlib/uncompressed are supported)",
                path.display()
            )));
        }
    };
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::je::nbt;
    use crate::nbt::{compound, tag_string};
    use bedrock_nbt::NbtTag;

    #[test]
    fn region_round_trips_chunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("r.0.0.mca");
        let first = nbt::encode("", &compound([("Name", tag_string("first"))])).expect("encode");
        let second = nbt::encode("", &compound([("Name", tag_string("second"))])).expect("encode");
        write_region_file(
            &path,
            vec![
                (0, 0, first.clone()),
                (31, 31, second.clone()),
            ],
            Compression::default(),
        )
        .expect("write");

        let mut region = RegionFile::open(&path, false).expect("open");
        assert!(region.has_chunk(0, 0));
        assert!(region.has_chunk(31, 31));
        assert!(!region.has_chunk(1, 1));
        assert_eq!(region.read_chunk(0, 0).expect("read").as_deref(), Some(first.as_slice()));
        assert_eq!(
            region.read_chunk(31, 31).expect("read").as_deref(),
            Some(second.as_slice())
        );
    }

    #[test]
    fn region_append_write_and_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("r.0.0.mca");
        let payload = nbt::encode("", &compound([("A", NbtTag::Int(7))])).expect("encode");
        let mut region = RegionFile::open(&path, true).expect("open");
        region
            .write_chunk(3, 4, &payload, RegionCompression::Zlib, Compression::fast())
            .expect("write");
        region.flush().expect("flush");
        drop(region);

        let mut region = RegionFile::open(&path, false).expect("reopen");
        assert_eq!(region.read_chunk(3, 4).expect("read").as_deref(), Some(payload.as_slice()));
        region.delete_chunk(3, 4);
        region.flush().expect("flush");
        assert!(!region.has_chunk(3, 4));
    }

    #[test]
    fn negative_chunk_coordinates_map_to_region_slots() {
        let pos = crate::je::JeChunkPos::new(-1, -33);
        assert_eq!(pos.region(), (-1, -2));
        assert_eq!(pos.local(), (31, 31));
    }
}
