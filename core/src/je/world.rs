//! Java world layout: `level.dat`, dimension folders, region enumeration and
//! chunk payload decoding.
//!
//! Both layouts are supported:
//! - pre-1.21: `region/`, `DIM-1/region/`, `DIM1/region/` under the world root;
//! - 1.21+: `dimensions/minecraft/{overworld,the_nether,the_end}/region/`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use bedrock_nbt::NbtTag;

use crate::error::{Result, WorldPortError, io_result};
use crate::je::anvil::{RegionFile, region_coordinates};
use crate::je::nbt::{self as je_nbt, NamedTag};
use crate::je::{JeChunkPos, JeDimension};
use crate::nbt::{NbtExt, TagValue};

/// Reuses open region-file handles across chunk reads.
///
/// Opening a region file re-reads its 8 KiB location/timestamp header. A bulk
/// reader that opens per chunk spends most of its time there: 11,192 chunks of
/// a real world took 5.77 s reopening versus 0.63 s with one handle per region
/// (16 opens). One cache per reader thread keeps handles private, so there is
/// no lock on the read path.
/// Region files a single reader keeps open at once.
///
/// A large world has hundreds of region files and every worker has its own
/// cache, so an unbounded map grows with the world size: thousands of file
/// handles plus one 8 KiB header each. Chunk reads are region-local, so a small
/// window keeps the hit rate while bounding the handles.
const REGION_CACHE_CAPACITY: usize = 24;

/// One cached region handle with the tick it was last used on.
struct CachedRegion {
    region: Option<RegionFile>,
    last_used: u64,
}

#[derive(Default)]
pub struct RegionCache {
    open: HashMap<PathBuf, CachedRegion>,
    clock: u64,
}

impl RegionCache {
    /// Reads one chunk payload through a cached region handle.
    pub fn read_chunk(
        &mut self,
        path: &Path,
        local_x: u8,
        local_z: u8,
    ) -> Result<Option<Vec<u8>>> {
        self.clock = self.clock.wrapping_add(1);
        let clock = self.clock;
        if !self.open.contains_key(path) {
            let region = match RegionFile::open_read_only(path) {
                Ok(region) => Some(region),
                Err(WorldPortError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            if self.open.len() >= REGION_CACHE_CAPACITY {
                // Evict the least recently used entry. Region reads are local,
                // so the entry about to be replaced is normally the coldest.
                if let Some(coldest) = self
                    .open
                    .iter()
                    .min_by_key(|(_, cached)| cached.last_used)
                    .map(|(key, _)| key.clone())
                {
                    self.open.remove(&coldest);
                }
            }
            self.open.insert(
                path.to_path_buf(),
                CachedRegion {
                    region,
                    last_used: clock,
                },
            );
        }
        let Some(cached) = self.open.get_mut(path) else {
            return Ok(None);
        };
        cached.last_used = clock;
        match cached.region.as_mut() {
            Some(region) => region.read_chunk(local_x, local_z),
            None => Ok(None),
        }
    }

    /// Region files currently held open.
    #[must_use]
    pub fn open_files(&self) -> usize {
        self.open.values().filter(|cached| cached.region.is_some()).count()
    }
}

/// Which on-disk layout the world uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldLayout {
    /// `dimensions/minecraft/<dimension>/` folders (1.21 and newer).
    Modern,
    /// `region/` plus `DIM-1`/`DIM1` folders.
    Legacy,
}

/// An opened Java world directory.
#[derive(Debug, Clone)]
pub struct JavaWorld {
    root: PathBuf,
    layout: WorldLayout,
    level_dat: NamedTag,
    world_gen_settings: Option<NbtTag>,
    game_rules: Option<NbtTag>,
    weather: Option<NbtTag>,
    world_clocks: Option<NbtTag>,
}

impl JavaWorld {
    /// Opens a world directory and parses `level.dat` plus the 26.x split data files.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let level_dat_path = root.join("level.dat");
        if !level_dat_path.is_file() {
            return Err(WorldPortError::invalid(format!(
                "{} is not a Java world (level.dat missing)",
                root.display()
            )));
        }
        let level_dat = je_nbt::read_gzip(&level_dat_path)?;
        if level_dat.tag.get("Data").is_none() {
            return Err(WorldPortError::invalid(
                "level.dat has no Data compound; this is not a Java Edition world",
            ));
        }
        let data_version = level_dat.tag.get("Data").and_then(|data| data.get_int("DataVersion")).unwrap_or(0);
        let layout = detect_layout(&root, data_version);
        let data_dir = root.join("data").join("minecraft");
        Ok(Self {
            world_gen_settings: read_split_data(&data_dir, "world_gen_settings"),
            game_rules: read_split_data(&data_dir, "game_rules"),
            weather: read_split_data(&data_dir, "weather"),
            world_clocks: read_split_data(&data_dir, "world_clocks"),
            root,
            layout,
            level_dat,
        })
    }

    /// `world_gen_settings.dat` payload (`data` compound) when the world uses the split layout.
    #[must_use]
    pub fn world_gen_settings(&self) -> Option<&NbtTag> {
        self.world_gen_settings.as_ref()
    }

    /// World directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Detected layout.
    #[must_use]
    pub const fn layout(&self) -> WorldLayout {
        self.layout
    }

    /// Raw `level.dat` root.
    #[must_use]
    pub const fn level_dat(&self) -> &NamedTag {
        &self.level_dat
    }

    /// The `Data` compound of `level.dat`.
    #[must_use]
    pub fn data(&self) -> &NbtTag {
        self.level_dat
            .tag
            .get("Data")
            .expect("open() validated the Data compound")
    }

    /// Mutable `Data` compound, for edition conversion.
    pub fn data_mut(&mut self) -> &mut NbtTag {
        self.level_dat
            .tag
            .get_mut("Data")
            .expect("open() validated the Data compound")
    }

    /// Chunk `DataVersion`, or `0` when absent (very old worlds).
    #[must_use]
    pub fn data_version(&self) -> i32 {
        self.data()
            .get_int("DataVersion")
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0)
    }

    /// Level name shown in the world list.
    #[must_use]
    pub fn level_name(&self) -> String {
        self.data()
            .get_str("LevelName")
            .map_or_else(|| default_level_name(&self.root), str::to_owned)
    }

    /// World seed, preferring the 26.x split `world_gen_settings.dat`, then
    /// `WorldGenSettings.seed`, then the legacy `RandomSeed`.
    #[must_use]
    pub fn seed(&self) -> i64 {
        self.world_gen_settings
            .as_ref()
            .and_then(|settings| settings.get_int("seed"))
            .or_else(|| {
                self.data()
                    .get("WorldGenSettings")
                    .and_then(|settings| settings.get_int("seed"))
            })
            .or_else(|| self.data().get_int("RandomSeed"))
            .unwrap_or(0)
    }

    /// Total elapsed simulation ticks (not the mutable daylight clock).
    #[must_use]
    pub fn total_ticks(&self) -> i64 {
        self.data().get_int("Time").unwrap_or_else(|| self.day_ticks())
    }

    /// Daylight ticks, including the 26.x split overworld clock.
    #[must_use]
    pub fn day_ticks(&self) -> i64 {
        if let Some(clocks) = &self.world_clocks {
            for dimension in ["minecraft:overworld", "overworld"] {
                if let Some(clock) = clocks.get(dimension)
                    && let Some(ticks) = clock.get_int("total_ticks")
                {
                    return ticks;
                }
            }
        }
        self.data()
            .get_int("DayTime")
            .or_else(|| self.data().get_int("Time"))
            .unwrap_or(0)
    }

    /// Whether it is currently raining.
    #[must_use]
    pub fn raining(&self) -> bool {
        self.weather
            .as_ref()
            .and_then(|weather| weather.get_bool("raining"))
            .or_else(|| self.data().get_bool("raining"))
            .unwrap_or(false)
    }

    /// Whether it is currently thundering.
    #[must_use]
    pub fn thundering(&self) -> bool {
        self.weather
            .as_ref()
            .and_then(|weather| weather.get_bool("thundering"))
            .or_else(|| self.data().get_bool("thundering"))
            .unwrap_or(false)
    }

    /// Remaining rain ticks.
    #[must_use]
    pub fn rain_time(&self) -> i64 {
        self.weather
            .as_ref()
            .and_then(|weather| weather.get_int("rain_time"))
            .or_else(|| self.data().get_int("rainTime"))
            .unwrap_or(0)
    }

    /// Remaining thunder ticks.
    #[must_use]
    pub fn thunder_time(&self) -> i64 {
        self.weather
            .as_ref()
            .and_then(|weather| weather.get_int("thunder_time"))
            .or_else(|| self.data().get_int("thunderTime"))
            .unwrap_or(0)
    }

    /// Game rules from either layout.
    ///
    /// Classic worlds store them as strings; the 26.x split file stores typed
    /// byte/int values, so the raw tags are returned and the target writer
    /// decides how to encode them.
    #[must_use]
    pub fn game_rules(&self) -> Vec<(String, NbtTag)> {
        let source = self
            .game_rules
            .as_ref()
            .or_else(|| self.data().get("GameRules"));
        let Some(compound) = source.and_then(|rules| rules.as_compound()) else {
            return Vec::new();
        };
        compound
            .iter()
            .filter(|(_, value)| {
                matches!(
                    value,
                    NbtTag::Byte(_)
                        | NbtTag::Int(_)
                        | NbtTag::Short(_)
                        | NbtTag::Long(_)
                        | NbtTag::Float(_)
                        | NbtTag::Double(_)
                        | NbtTag::String(_)
                )
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    /// Dimension top-level folder (the directory that contains `region/`).
    #[must_use]
    pub fn dimension_dir(&self, dimension: JeDimension) -> PathBuf {
        match self.layout {
            WorldLayout::Modern => self
                .root
                .join("dimensions")
                .join("minecraft")
                .join(dimension.folder_name()),
            WorldLayout::Legacy => {
                let legacy = dimension.legacy_folder_name();
                if legacy.is_empty() {
                    self.root.clone()
                } else {
                    self.root.join(legacy)
                }
            }
        }
    }

    /// Region folder for a dimension.
    #[must_use]
    pub fn region_dir(&self, dimension: JeDimension) -> PathBuf {
        self.dimension_dir(dimension).join("region")
    }

    /// Entity region folder for a dimension (1.17+ layout).
    #[must_use]
    pub fn entity_region_dir(&self, dimension: JeDimension) -> PathBuf {
        self.dimension_dir(dimension).join("entities")
    }

    /// Reads the entities stored in a separate `entities/r.X.Z.mca` chunk.
    ///
    /// The 1.17+ format keeps `{ Entities, DataVersion, Position }` per chunk.
    pub fn read_entity_chunk(
        &self,
        dimension: JeDimension,
        pos: JeChunkPos,
    ) -> Result<Vec<NbtTag>> {
        let mut cache = RegionCache::default();
        self.read_entity_chunk_cached(&mut cache, dimension, pos)
    }

    /// Reads a separate `entities/` chunk, reusing region handles from `cache`.
    pub fn read_entity_chunk_cached(
        &self,
        cache: &mut RegionCache,
        dimension: JeDimension,
        pos: JeChunkPos,
    ) -> Result<Vec<NbtTag>> {
        let (region_x, region_z) = pos.region();
        let path = self
            .entity_region_dir(dimension)
            .join(format!("r.{region_x}.{region_z}.mca"));
        let (local_x, local_z) = pos.local();
        let Some(payload) = cache.read_chunk(&path, local_x, local_z)? else {
            return Ok(Vec::new());
        };
        let NbtTag::Compound(mut root) = je_nbt::decode(&payload)?.tag else {
            return Ok(Vec::new());
        };
        match root.swap_remove("Entities") {
            Some(NbtTag::List(entities)) => Ok(entities),
            _ => Ok(Vec::new()),
        }
    }

    /// Existing region files for a dimension as `(region_x, region_z, path)`.
    pub fn region_files(&self, dimension: JeDimension) -> Result<Vec<(i32, i32, PathBuf)>> {
        let dir = self.region_dir(dimension);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in io_result(&dir, std::fs::read_dir(&dir))? {
            let entry = io_result(&dir, entry)?;
            let path = entry.path();
            if let Some((x, z)) = region_coordinates(&path) {
                files.push((x, z, path));
            }
        }
        files.sort_by_key(|(x, z, _)| (*x, *z));
        Ok(files)
    }

    /// Every chunk present in a dimension, from region headers only.
    pub fn chunk_positions(&self, dimension: JeDimension) -> Result<Vec<JeChunkPos>> {
        let mut positions = Vec::new();
        let mut seen = HashSet::new();
        for (region_x, region_z, path) in self.region_files(dimension)? {
            let region = RegionFile::open_read_only(&path)?;
            for (local_x, local_z) in region.occupied_slots() {
                let pos = JeChunkPos::new(
                    region_x * 32 + i32::from(local_x),
                    region_z * 32 + i32::from(local_z),
                );
                if seen.insert(pos) {
                    positions.push(pos);
                }
            }
        }
        Ok(positions)
    }

    /// Reads and decodes one chunk's NBT root compound.
    pub fn read_chunk(&self, dimension: JeDimension, pos: JeChunkPos) -> Result<Option<NbtTag>> {
        let mut cache = RegionCache::default();
        self.read_chunk_cached(&mut cache, dimension, pos)
    }

    /// Reads and decodes one chunk, reusing region handles from `cache`.
    pub fn read_chunk_cached(
        &self,
        cache: &mut RegionCache,
        dimension: JeDimension,
        pos: JeChunkPos,
    ) -> Result<Option<NbtTag>> {
        let (region_x, region_z) = pos.region();
        // The region directory is resolved once per world, but the file name
        // still has to be built per chunk. `is_file()` was a `stat` per chunk
        // (one syscall per block column); a missing region is already reported
        // by the open attempt, so the probe is gone.
        let path = self
            .region_dir(dimension)
            .join(format!("r.{region_x}.{region_z}.mca"));
        let (local_x, local_z) = pos.local();
        let payload = match cache.read_chunk(&path, local_x, local_z)? {
            Some(payload) => Some(payload),
            None if self.layout == WorldLayout::Legacy => {
                cache.read_chunk(&path.with_extension("mcr"), local_x, local_z)?
            }
            None => None,
        };
        let Some(payload) = payload else {
            return Ok(None);
        };
        let decoded = je_nbt::decode(&payload)?;
        Ok(Some(decoded.tag))
    }

    /// Player data files as `(file stem, path)`, ordered by name.
    pub fn player_files(&self) -> Result<Vec<(String, PathBuf)>> {
        read_dat_stems(&self.player_data_dir())
    }

    /// The local player's NBT, wherever this world keeps it.
    ///
    /// Modern worlds store it in `playerdata/<uuid>.dat`; classic ones embed it
    /// in `level.dat` as `Data.Player`. Comparison and conversion both need the
    /// player as a concept, not as a file, because a conversion legitimately
    /// moves it from one place to the other.
    #[must_use]
    pub fn local_player(&self) -> Option<NbtTag> {
        if let Ok(files) = self.player_files()
            && let Some((_, path)) = files.first()
            && let Ok(named) = je_nbt::read_gzip(path)
        {
            return Some(named.tag);
        }
        self.data().get("Player").cloned()
    }

    /// Directory holding map data files.
    ///
    /// 26.x moved them to `data/minecraft/maps/`; older worlds keep them in `data/`.
    #[must_use]
    pub fn maps_dir(&self) -> PathBuf {
        let modern = self.root.join("data").join("minecraft").join("maps");
        if modern.is_dir() {
            modern
        } else {
            self.root.join("data")
        }
    }

    /// Player data directory (`players/data/` in 26.x, `playerdata/` before).
    #[must_use]
    pub fn player_data_dir(&self) -> PathBuf {
        let modern = self.root.join("players").join("data");
        if modern.is_dir() {
            modern
        } else {
            self.root.join("playerdata")
        }
    }

    /// Map item files as `(map id, path)`, ordered by id.
    ///
    /// Bedrock map ids are 64-bit and Java's are 32-bit, so the id is read as
    /// `i64` and normalised by the caller when writing Java.
    pub fn map_files(&self) -> Result<Vec<(i64, PathBuf)>> {
        // 26.1+ stores data/minecraft/maps/<id>.dat; older versions use
        // data/map_<id>.dat. Prefer the modern copy when both exist.
        let modern = self.root.join("data").join("minecraft").join("maps");
        let mut directories = Vec::with_capacity(2);
        if modern.is_dir() {
            directories.push((modern, true));
        }
        directories.push((self.root.join("data"), false));
        let mut maps: BTreeMap<i64, PathBuf> = BTreeMap::new();
        for (dir, numbered) in directories {
            for (stem, path) in read_dat_stems(&dir)? {
                let id = stem.strip_prefix("map_")
                    .and_then(|id| id.parse::<i64>().ok())
                    .or_else(|| numbered.then(|| stem.parse::<i64>().ok()).flatten());
                if let Some(id) = id {
                    maps.entry(id).or_insert(path);
                }
            }
        }
        Ok(maps.into_iter().collect())
    }
}

fn detect_layout(root: &Path, data_version: i64) -> WorldLayout {
    // A pre-26.1 world may have a dimensions directory for custom dimensions.
    // Prefer its actual vanilla layout; retain compatibility with old WorldPort
    // outputs which incorrectly used the 26.1 paths for earlier versions.
    let legacy = ["region", "DIM-1", "DIM1"].iter().any(|name| root.join(name).is_dir());
    let modern = ["overworld", "the_nether", "the_end"].iter().any(|name|
        root.join("dimensions").join("minecraft").join(name).is_dir());
    if data_version >= 4786 || (!legacy && modern) {
        WorldLayout::Modern
    } else {
        WorldLayout::Legacy
    }
}

/// Reads a 26.x split data file (`data/minecraft/<name>.dat`) and returns its
/// `data` compound. Missing or malformed files yield `None` so older worlds keep
/// using the `level.dat` fields.
fn read_split_data(dir: &Path, name: &str) -> Option<NbtTag> {
    let path = dir.join(format!("{name}.dat"));
    if !path.is_file() {
        return None;
    }
    let document = je_nbt::read_gzip(&path).ok()?;
    document
        .tag
        .get("data")
        .cloned()
        .or_else(|| Some(document.tag.clone()))
}

fn read_dat_stems(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in io_result(dir, std::fs::read_dir(dir))? {
        let entry = io_result(dir, entry)?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "dat")
            && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        {
            files.push((stem.to_string(), path));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}


fn default_level_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("world")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_numbered_26x_maps_without_dropping_legacy_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        je_nbt::write_gzip(
            &dir.path().join("level.dat"),
            "",
            &crate::nbt::compound([("Data", crate::nbt::empty_compound())]),
        ).expect("level.dat");
        let modern = dir.path().join("data").join("minecraft").join("maps");
        std::fs::create_dir_all(&modern).expect("modern map directory");
        let legacy = dir.path().join("data");
        std::fs::write(modern.join("7.dat"), b"modern").expect("modern map");
        std::fs::write(legacy.join("map_3.dat"), b"legacy").expect("legacy map");
        std::fs::write(legacy.join("map_7.dat"), b"duplicate").expect("duplicate map");
        let world = JavaWorld::open(dir.path()).expect("world");
        let files = world.map_files().expect("maps");
        assert_eq!(files.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![3, 7]);
        assert_eq!(files[1].1, modern.join("7.dat"));
    }

    #[test]
    fn parses_region_file_names() {
        assert_eq!(
            region_coordinates(Path::new("r.-3.12.mca")),
            Some((-3, 12))
        );
        assert_eq!(region_coordinates(Path::new("r.0.mca")), None);
        assert_eq!(region_coordinates(Path::new("level.dat")), None);
    }
}
