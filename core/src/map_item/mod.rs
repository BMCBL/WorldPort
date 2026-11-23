//! In-game map item conversion.
//!
//! Java stores one byte per pixel (`base * 4 + shade`) in `map_<id>.dat`;
//! Bedrock stores raw RGBA (`colors`, 4 bytes per pixel) in the `map_<id>`
//! LevelDB record. Both use a 128x128 grid by default.

use std::path::{Path, PathBuf};

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::error::{Result, io_result};
use crate::je::nbt as je_nbt;
use crate::mapping::map_colors::{java_index_to_rgba, rgba_to_java_index, shaded_palette};
use crate::nbt::{NbtExt, TagValue, compound, empty_compound, tag_string};

/// One in-game map.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapData {
    /// Map id (`map_<id>` / `map_<id>.dat`).
    pub id: i64,
    /// Zoom scale, 0..=4.
    pub scale: i8,
    /// Dimension identifier (`minecraft:overworld`).
    pub dimension: String,
    /// Centre block X.
    pub x_center: i32,
    /// Centre block Z.
    pub z_center: i32,
    /// Pixel width.
    pub width: i16,
    /// Pixel height.
    pub height: i16,
    /// Locked maps cannot be modified in-game.
    pub locked: bool,
    /// Track player positions.
    pub tracking: bool,
    /// Java palette indices, `width * height` entries.
    pub colors: Vec<u8>,
}

/// Java map colour byte → RGBA.
#[must_use]
pub fn java_colors_to_rgba(colors: &[u8], palette: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(colors.len() * 4);
    for index in colors {
        out.extend_from_slice(&java_index_to_rgba(*index, palette));
    }
    out
}

/// Bedrock RGBA → nearest Java palette bytes.
///
/// Real maps repeat a small number of colours (terrain palettes), so exact
/// per-colour memoisation removes almost all of the 248-entry nearest search.
/// `worldport bench-colors` measures this against the plain scalar loop; a
/// fearless_simd kernel was prototyped but the memo hit rate leaves it no
/// headroom, so the scalar path stays (measure before optimising).
#[must_use]
pub fn rgba_to_java_colors(rgba: &[u8], palette: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    let mut memo: rustc_hash::FxHashMap<u32, u8> = rustc_hash::FxHashMap::default();
    for pixel in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([pixel[0], pixel[1], pixel[2], pixel[3]]);
        let index = *memo
            .entry(key)
            .or_insert_with(|| rgba_to_java_index([pixel[0], pixel[1], pixel[2], pixel[3]], palette));
        out.push(index);
    }
    out
}

/// Plain scalar conversion (no memo), used as the benchmark baseline.
#[must_use]
pub fn rgba_to_java_colors_scalar(rgba: &[u8], palette: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    for pixel in rgba.chunks_exact(4) {
        out.push(rgba_to_java_index(
            [pixel[0], pixel[1], pixel[2], pixel[3]],
            palette,
        ));
    }
    out
}

/// Reads a Java `map_<id>.dat` file.
pub fn read_java_map(path: &Path, id: i64) -> Result<MapData> {
    let document = je_nbt::read_gzip(path)?;
    let data = document
        .tag
        .get("data")
        .or_else(|| Some(&document.tag))
        .expect("root tag");
    let dimension = match data.get("dimension") {
        Some(NbtTag::String(name)) => name.clone(),
        Some(NbtTag::Byte(id)) => dimension_name(i64::from(*id)).to_string(),
        Some(NbtTag::Int(id)) => dimension_name(i64::from(*id)).to_string(),
        _ => "minecraft:overworld".to_string(),
    };
    let palette = shaded_palette();
    let colors = data
        .get("colors")
        .and_then(|colors| match colors {
            NbtTag::ByteArray(bytes) => Some(bytes.iter().map(|byte| *byte as u8).collect()),
            _ => None,
        })
        .unwrap_or_default();
    let _ = &palette;
    Ok(MapData {
        id,
        scale: data.get_int("scale").unwrap_or(0) as i8,
        dimension,
        x_center: data.get_int("xCenter").unwrap_or(0) as i32,
        z_center: data.get_int("zCenter").unwrap_or(0) as i32,
        width: data.get_int("width").unwrap_or(128) as i16,
        height: data.get_int("height").unwrap_or(128) as i16,
        locked: data.get_bool("locked").unwrap_or(false),
        tracking: data.get_bool("unlimitedTracking").unwrap_or(false),
        colors,
    })
}

/// Writes Java `map_<id>.dat` (pre-26.1) or numbered `<id>.dat` (26.1+).
pub fn write_java_map(
    dir: &Path,
    map: &MapData,
    data_version: i32,
    dimension_string: bool,
) -> Result<PathBuf> {
    io_result(dir, std::fs::create_dir_all(dir))?;
    let dimension = if dimension_string {
        tag_string(map.dimension.clone())
    } else {
        NbtTag::Byte(dimension_id(&map.dimension) as i8)
    };
    let data = compound([
        ("scale", NbtTag::Byte(map.scale)),
        ("dimension", dimension),
        ("xCenter", NbtTag::Int(map.x_center)),
        ("zCenter", NbtTag::Int(map.z_center)),
        ("width", NbtTag::Short(map.width)),
        ("height", NbtTag::Short(map.height)),
        ("locked", NbtTag::Byte(i8::from(map.locked))),
        (
            "trackingPosition",
            NbtTag::Byte(i8::from(map.tracking)),
        ),
        (
            "unlimitedTracking",
            NbtTag::Byte(i8::from(map.tracking)),
        ),
        ("banners", NbtTag::List(Vec::new())),
        (
            "colors",
            NbtTag::ByteArray(map.colors.iter().map(|byte| *byte as i8).collect()),
        ),
    ]);
    let root = compound([
        ("data", data),
        ("DataVersion", NbtTag::Int(data_version)),
    ]);
    // Java 26.1 moved maps to data/minecraft/maps/<id>.dat.
    let filename = if data_version >= 4786 {
        format!("{}.dat", map.id)
    } else {
        format!("map_{}.dat", map.id)
    };
    let path = dir.join(filename);
    je_nbt::write_gzip(&path, "", &root)?;
    Ok(path)
}

/// Writes the Java map ID counter in the target version's format.
///
/// Both modern `last_id.dat` and legacy `idcounts.dat` use GZip NBT.
/// The map counter has used TAG_Int since the 1.13 era, not TAG_Short.
pub fn write_idcounts(dir: &Path, max_id: i64, data_version: i32) -> Result<()> {
    io_result(dir, std::fs::create_dir_all(dir))?;
    let id = i32::try_from(max_id)
        .map_err(|_| crate::error::WorldPortError::other("Java map counter exceeds i32"))?;
    let root = compound([("map", NbtTag::Int(id))]);
    let name = if data_version >= 4786 { "last_id.dat" } else { "idcounts.dat" };
    je_nbt::write_gzip(&dir.join(name), "", &root)
}

/// Reads a Bedrock `map_<id>` record.
#[must_use]
pub fn read_bedrock_map(nbt: &NbtTag, id: i64) -> Option<MapData> {
    let palette = shaded_palette();
    let dimension = match nbt.get("dimension") {
        Some(NbtTag::String(name)) => name.clone(),
        Some(value) => dimension_name(value.as_i64_value().unwrap_or(0)).to_string(),
        None => "minecraft:overworld".to_string(),
    };
    let rgba = match nbt.get("colors") {
        Some(NbtTag::ByteArray(bytes)) => bytes.iter().map(|byte| *byte as u8).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    Some(MapData {
        id,
        scale: nbt.get_int("scale").unwrap_or(0) as i8,
        dimension,
        x_center: nbt.get_int("xCenter").unwrap_or(0) as i32,
        z_center: nbt.get_int("zCenter").unwrap_or(0) as i32,
        width: nbt.get_int("width").unwrap_or(128) as i16,
        height: nbt.get_int("height").unwrap_or(128) as i16,
        locked: nbt.get_bool("mapLocked").unwrap_or(false),
        tracking: nbt.get_bool("unlimitedTracking").unwrap_or(false),
        colors: rgba_to_java_colors(&rgba, &palette),
    })
}

/// Builds a Bedrock `map_<id>` record.
#[must_use]
pub fn write_bedrock_map(map: &MapData) -> NbtTag {
    let palette = shaded_palette();
    let rgba = java_colors_to_rgba(&map.colors, &palette);
    let mut root = IndexMap::new();
    root.insert("map_uuid".to_string(), NbtTag::Long(map.id));
    root.insert(
        "dimension".to_string(),
        NbtTag::Byte(dimension_id(&map.dimension) as i8),
    );
    root.insert("scale".to_string(), NbtTag::Byte(map.scale));
    root.insert("xCenter".to_string(), NbtTag::Int(map.x_center));
    root.insert("zCenter".to_string(), NbtTag::Int(map.z_center));
    root.insert("width".to_string(), NbtTag::Short(map.width));
    root.insert("height".to_string(), NbtTag::Short(map.height));
    root.insert(
        "unlimitedTracking".to_string(),
        NbtTag::Byte(i8::from(map.tracking)),
    );
    root.insert(
        "mapLocked".to_string(),
        NbtTag::Byte(i8::from(map.locked)),
    );
    root.insert("decorations".to_string(), empty_compound());
    root.insert(
        "colors".to_string(),
        NbtTag::ByteArray(rgba.iter().map(|byte| *byte as i8).collect()),
    );
    NbtTag::Compound(root)
}

/// Java dimension id for a Bedrock map dimension byte.
#[must_use]
pub const fn dimension_name(id: i64) -> &'static str {
    match id {
        1 => "minecraft:the_nether",
        2 => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

/// Bedrock map dimension byte for a Java dimension identifier.
#[must_use]
pub fn dimension_id(name: &str) -> i32 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "the_nether" => 1,
        "the_end" => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_map() -> MapData {
        MapData {
            id: 7,
            scale: 1,
            dimension: "minecraft:the_nether".to_string(),
            x_center: 64,
            z_center: -32,
            width: 128,
            height: 128,
            locked: true,
            tracking: true,
            colors: (0..16384).map(|index| (index % 248) as u8).collect(),
        }
    }

    #[test]
    fn bedrock_map_round_trips_colours() {
        let map = sample_map();
        let nbt = write_bedrock_map(&map);
        let back = read_bedrock_map(&nbt, map.id).expect("map");
        assert_eq!(back.dimension, map.dimension);
        assert_eq!(back.x_center, 64);
        assert!(back.locked);
        assert_eq!(back.colors.len(), 16384);
        // Several palette entries share one RGBA value (transparent black), so
        // compare rendered colours rather than raw indices.
        let palette = shaded_palette();
        assert_eq!(
            java_colors_to_rgba(&back.colors, &palette),
            java_colors_to_rgba(&map.colors, &palette)
        );
    }

    #[test]
    fn java_map_counter_uses_compressed_int_and_versioned_name() {
        for (version, filename) in [(4189, "idcounts.dat"), (4903, "last_id.dat")] {
            let dir = tempfile::tempdir().expect("tempdir");
            write_idcounts(dir.path(), 65_540, version).expect("map counter");
            let path = dir.path().join(filename);
            let root = je_nbt::read_gzip(&path).expect("gzip map counter").tag;
            assert_eq!(root.get("map"), Some(&NbtTag::Int(65_540)));
        }
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(write_idcounts(dir.path(), i64::from(i32::MAX) + 1, 4903).is_err());
    }

    #[test]
    fn legacy_map_filename_is_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_java_map(dir.path(), &sample_map(), 4189, true).expect("write");
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("map_7.dat"));
    }

    #[test]
    fn java_map_file_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = sample_map();
        let path = write_java_map(dir.path(), &map, 4903, true).expect("write");
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("7.dat"));
        let back = read_java_map(&path, map.id).expect("read");
        assert_eq!(back.id, map.id);
        assert_eq!(back.scale, map.scale);
        assert_eq!(back.dimension, map.dimension);
        assert_eq!(back.x_center, map.x_center);
        assert_eq!(back.z_center, map.z_center);
        assert_eq!(back.width, map.width);
        assert_eq!(back.height, map.height);
        assert_eq!(back.locked, map.locked);
        assert_eq!(back.tracking, map.tracking);
        assert_eq!(back.colors, map.colors, "map files keep the exact indices");
    }
}
