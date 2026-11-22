//! Java world layout: `level.dat`, dimension folders, region enumeration and
//! chunk payload decoding.
//!
//! Both layouts are supported:
//! - pre-1.21: `region/`, `DIM-1/region/`, `DIM1/region/` under the world root;
//! - 1.21+: `dimensions/minecraft/{overworld,the_nether,the_end}/region/`.

use std::path::{Path, PathBuf};

use bedrock_nbt::NbtTag;

use crate::error::{Result, WorldPortError, io_result};
use crate::je::anvil::RegionFile;
use crate::je::nbt::{self as je_nbt, NamedTag};
use crate::je::{JeChunkPos, JeDimension};
use crate::nbt::{NbtExt, TagValue};

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
        let layout = detect_layout(&root);
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

    /// Total game ticks for a dimension; 26.x keeps them in `world_clocks.dat`.
    #[must_use]
    pub fn total_ticks(&self) -> i64 {
        if let Some(clocks) = &self.world_clocks {
            for dimension in ["minecraft:overworld", "overworld"] {
                if let Some(clock) = clocks.get(dimension)
                    && let Some(ticks) = clock.get_int("total_ticks")
                {
                    return ticks;
                }
            }
        }
        self.data().get_int("DayTime").unwrap_or(0)
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
        let (region_x, region_z) = pos.region();
        let path = self
            .entity_region_dir(dimension)
            .join(format!("r.{region_x}.{region_z}.mca"));
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let mut region = RegionFile::open(&path, false)?;
        let (local_x, local_z) = pos.local();
        let Some(payload) = region.read_chunk(local_x, local_z)? else {
            return Ok(Vec::new());
        };
        let decoded = je_nbt::decode(&payload)?;
        Ok(decoded
            .tag
            .get_list("Entities")
            .map(<[NbtTag]>::to_vec)
            .unwrap_or_default())
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
            if path.extension().is_some_and(|ext| ext == "mca")
                && let Some((x, z)) = parse_region_file_name(&path)
            {
                files.push((x, z, path));
            }
        }
        files.sort_by_key(|(x, z, _)| (*x, *z));
        Ok(files)
    }

    /// Every chunk present in a dimension, from region headers only.
    pub fn chunk_positions(&self, dimension: JeDimension) -> Result<Vec<JeChunkPos>> {
        let mut positions = Vec::new();
        for (region_x, region_z, path) in self.region_files(dimension)? {
            let region = RegionFile::open(&path, false)?;
            for (local_x, local_z) in region.occupied_slots() {
                positions.push(JeChunkPos::new(
                    region_x * 32 + i32::from(local_x),
                    region_z * 32 + i32::from(local_z),
                ));
            }
        }
        Ok(positions)
    }

    /// Reads and decodes one chunk's NBT root compound.
    pub fn read_chunk(&self, dimension: JeDimension, pos: JeChunkPos) -> Result<Option<NbtTag>> {
        let (region_x, region_z) = pos.region();
        let path = self
            .region_dir(dimension)
            .join(format!("r.{region_x}.{region_z}.mca"));
        if !path.is_file() {
            return Ok(None);
        }
        let mut region = RegionFile::open(&path, false)?;
        let (local_x, local_z) = pos.local();
        let Some(payload) = region.read_chunk(local_x, local_z)? else {
            return Ok(None);
        };
        let decoded = je_nbt::decode(&payload)?;
        Ok(Some(decoded.tag))
    }

    /// Player data files as `(file stem, path)`, ordered by name.
    pub fn player_files(&self) -> Result<Vec<(String, PathBuf)>> {
        read_dat_stems(&self.root.join("playerdata"))
    }

    /// Map item files as `(map id, path)`, ordered by id.
    pub fn map_files(&self) -> Result<Vec<(i32, PathBuf)>> {
        let dir = self.root.join("data");
        let mut maps = Vec::new();
        for (stem, path) in read_dat_stems(&dir)? {
            if let Some(id) = stem.strip_prefix("map_").and_then(|id| id.parse::<i32>().ok()) {
                maps.push((id, path));
            }
        }
        maps.sort_by_key(|(id, _)| *id);
        Ok(maps)
    }
}

fn detect_layout(root: &Path) -> WorldLayout {
    if root.join("dimensions").is_dir() {
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

fn parse_region_file_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_stem()?.to_str()?;
    let mut parts = name.split('.');
    if parts.next()? != "r" {
        return None;
    }
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((x, z))
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
    fn parses_region_file_names() {
        assert_eq!(
            parse_region_file_name(Path::new("r.-3.12.mca")),
            Some((-3, 12))
        );
        assert_eq!(parse_region_file_name(Path::new("r.0.mca")), None);
        assert_eq!(parse_region_file_name(Path::new("level.dat")), None);
    }
}
