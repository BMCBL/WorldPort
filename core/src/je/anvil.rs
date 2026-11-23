//! Java region containers (`.mca` and `.mcr`) with 4 KiB sectors.
//! Reads gzip, zlib, raw NBT, Java's LZ4 block stream and external `.mcc` chunks.
//!
//! Reading tolerates files produced by third-party tools; writing always emits a
//! compact contiguous region. Bulk conversion uses [`write_region_file`], which
//! streams chunks and patches the header at the end.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::Compression;
use rayon::prelude::*;

use crate::error::{Result, WorldPortError, io_result};

mod compression;
pub use compression::compress;
use compression::decompress;

const SECTOR_BYTES: u64 = 4096;
const HEADER_BYTES: u64 = SECTOR_BYTES * 2;
const SLOTS: usize = 1024;
const EXTERNAL_CHUNK: u8 = 0x80;
const MAX_EXTERNAL_BYTES: u64 = crate::je::MAX_PAYLOAD_BYTES + 1024 * 1024;

/// Returns the coordinates from a standard `r.<x>.<z>.mca` or `.mcr` name.
#[must_use]
pub fn region_coordinates(path: &Path) -> Option<(i32, i32)> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "mca" | "mcr") {
        return None;
    }
    let mut parts = path.file_stem()?.to_str()?.split('.');
    if parts.next()? != "r" {
        return None;
    }
    let x: i32 = parts.next()?.parse().ok()?;
    let z: i32 = parts.next()?.parse().ok()?;
    x.checked_mul(32)?;
    z.checked_mul(32)?;
    parts.next().is_none().then_some((x, z))
}

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
    /// Opens a region file for reading only.
    ///
    /// A read-only world directory, a read-only medium or a file without write
    /// permission could not be opened by [`Self::open`], which requests write
    /// access because it also serves writers.
    pub fn open_read_only(path: &Path) -> Result<Self> {
        let file = io_result(path, File::options().read(true).open(path))?;
        let length = io_result(path, file.metadata())?.len();
        let mut this = Self {
            path: path.to_path_buf(),
            file,
            locations: [Location::default(); SLOTS],
            timestamps: [0; SLOTS],
            header_dirty: false,
            next_sector: u32::try_from(length.div_ceil(SECTOR_BYTES)).unwrap_or(u32::MAX),
        };
        this.read_header()?;
        Ok(this)
    }

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
            // Readers and writers share this constructor, and a writer appends
            // to an existing file, so write access is requested here. A
            // read-only source needs `open_read_only`.
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
        local_x < 32 && local_z < 32 && self.locations[slot_index(local_x, local_z)].is_present()
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
        if local_x >= 32 || local_z >= 32 {
            return Err(WorldPortError::invalid(format!(
                "chunk position {local_x},{local_z} is outside the region"
            )));
        }
        let location = self.locations[slot_index(local_x, local_z)];
        if !location.is_present() {
            return Ok(None);
        }
        if location.offset_sectors < 2 {
            return Err(WorldPortError::invalid("chunk overlaps the region header"));
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
        .map_err(|_| WorldPortError::invalid("chunk length overflow"))?;
        if length == 0 {
            return Ok(None);
        }
        let compression = header[4];
        let payload_len = length - 1;
        // The declared length must fit the sectors the header allocated for
        // this chunk. Without this check a corrupt or hostile `.mca` can make
        // the reader allocate an arbitrary buffer before the read fails.
        let capacity = u64::from(location.sector_count) * SECTOR_BYTES;
        if (length as u64).saturating_add(4) > capacity {
            return Err(WorldPortError::invalid(format!(
                "chunk at {},{} declares {length} bytes but its header allocates {capacity}",
                local_x, local_z
            )));
        }
        if compression & EXTERNAL_CHUNK != 0 {
            if length != 1 {
                return Err(WorldPortError::invalid(
                    "external chunk stub must contain only the compression ID",
                ));
            }
            return self
                .read_external_chunk(local_x, local_z, compression & !EXTERNAL_CHUNK)
                .map(Some);
        }
        let mut compressed = vec![0_u8; payload_len];
        self.file
            .read_exact(&mut compressed)
            .map_err(|error| WorldPortError::io(&self.path, error))?;
        decompress(&self.path, compression, compressed).map(Some)
    }

    fn read_external_chunk(&self, local_x: u8, local_z: u8, compression: u8) -> Result<Vec<u8>> {
        let (x, z) = region_coordinates(&self.path).ok_or_else(|| {
            WorldPortError::invalid(format!("invalid region name: {}", self.path.display()))
        })?;
        let path = self.path.with_file_name(format!(
            "c.{}.{}.mcc",
            x * 32 + i32::from(local_x),
            z * 32 + i32::from(local_z)
        ));
        let file = io_result(&path, File::open(&path))?;
        let size = io_result(&path, file.metadata())?.len();
        if size > MAX_EXTERNAL_BYTES {
            return Err(WorldPortError::invalid(format!(
                "external chunk is too large: {}",
                path.display()
            )));
        }
        let mut data = Vec::with_capacity(size as usize);
        io_result(
            &path,
            file.take(MAX_EXTERNAL_BYTES + 1).read_to_end(&mut data),
        )?;
        if data.len() as u64 > MAX_EXTERNAL_BYTES {
            return Err(WorldPortError::invalid(format!(
                "external chunk is too large: {}",
                path.display()
            )));
        }
        decompress(&path, compression, data)
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
        if local_x >= 32 || local_z >= 32 {
            return Err(WorldPortError::invalid(format!(
                "chunk position {local_x},{local_z} is outside the region"
            )));
        }
        let compressed = compress(compression, payload, level)?;
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| WorldPortError::invalid("chunk payload exceeds 4 GiB"))?;
        let span = u64::from(length) + 4;
        let sectors = u8::try_from(span.div_ceil(SECTOR_BYTES))
            .map_err(|_| WorldPortError::invalid("chunk payload exceeds 255 sectors"))?;
        let index = slot_index(local_x, local_z);
        let existing = self.locations[index];
        let offset = if existing.sector_count == sectors {
            existing.offset_sectors
        } else {
            let offset = self.next_sector;
            self.next_sector = self
                .next_sector
                .checked_add(u32::from(sectors))
                .ok_or_else(|| WorldPortError::invalid("region file exceeds 4 GiB"))?;
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
        // A zero-length region file is a normal thing for a game to leave
        // behind (an entities directory that never received a chunk); treat it
        // as an empty region rather than an I/O error.
        let length = io_result(&self.path, self.file.metadata())?.len();
        if length == 0 {
            return Ok(());
        }
        if length < HEADER_BYTES {
            return Err(WorldPortError::invalid(format!(
                "region file {} is {} bytes, shorter than the {HEADER_BYTES}-byte header",
                self.path.display(),
                length
            )));
        }
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
            ZlibEncode::Payload(bytes) => compress(RegionCompression::Zlib, &bytes, level)
                .map(|compressed| (local_x, local_z, compressed)),
        })
        .collect::<Result<Vec<_>>>()?;
    const ZERO_SECTOR: [u8; SECTOR_BYTES as usize] = [0; SECTOR_BYTES as usize];
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    writer
        .write_all(&[0_u8; HEADER_BYTES as usize])
        .map_err(|error| WorldPortError::io(path, error))?;
    let mut locations = [Location::default(); SLOTS];
    let timestamp = unix_now_seconds();
    let mut next_sector = 2_u32;
    for (local_x, local_z, compressed) in compressed {
        if local_x >= 32 || local_z >= 32 {
            return Err(WorldPortError::invalid(format!(
                "local chunk position out of range: {local_x},{local_z}"
            )));
        }
        let length = u32::try_from(compressed.len() + 1)
            .map_err(|_| WorldPortError::invalid("chunk payload exceeds 4 GiB"))?;
        let span = u64::from(length) + 4;
        let sectors = u8::try_from(span.div_ceil(SECTOR_BYTES))
            .map_err(|_| WorldPortError::invalid("chunk payload exceeds 255 sectors"))?;
        locations[slot_index(local_x, local_z)] = Location {
            offset_sectors: next_sector,
            sector_count: sectors,
        };
        next_sector = next_sector
            .checked_add(u32::from(sectors))
            .ok_or_else(|| WorldPortError::invalid("region file exceeds 4 GiB"))?;
        writer
            .write_all(&length.to_be_bytes())
            .and_then(|()| writer.write_all(&[RegionCompression::Zlib.id()]))
            .and_then(|()| writer.write_all(&compressed))
            .map_err(|error| WorldPortError::io(path, error))?;
        let padding = u64::from(sectors) * SECTOR_BYTES - span;
        if padding > 0 {
            writer
                .write_all(&ZERO_SECTOR[..padding as usize])
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

#[cfg(test)]
mod tests;
