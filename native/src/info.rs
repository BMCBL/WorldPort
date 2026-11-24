//! Lightweight world metadata and original save icons for the plugin.

use std::path::Path;

use worldport_core::convert::{Edition, detect_edition};
use worldport_core::error::{Result, WorldPortError};
use worldport_core::nbt::NbtExt;
use worldport_protocol::WorldInfo;

/// Inspect only the world header and icon. Never open LevelDB or enumerate chunks.
pub fn world_info(path: &Path) -> Result<WorldInfo> {
    let edition = detect_edition(path)?;
    let mut info = match edition {
        Edition::Java => {
            let document = worldport_core::je::nbt::read_gzip(&path.join("level.dat"))?;
            let data = document.tag.get("Data")
                .ok_or_else(|| WorldPortError::invalid("Java level.dat missing Data"))?;
            let data_version = data.get_int("DataVersion").unwrap_or(0);
            // Report the actual save version, not the closest conversion catalog.
            let version = data.get("Version").and_then(|version| version.get_str("Name"))
                .filter(|name| !name.is_empty()).map(str::to_owned)
                .unwrap_or_else(|| format!("DataVersion {data_version}"));
            WorldInfo {
                edition: "java".into(),
                level_name: data.get_str("LevelName").unwrap_or_default().to_owned(),
                version,
                ..WorldInfo::default()
            }
        }
        Edition::Bedrock => {
            let summary = worldport_core::be::inspect::read_metadata(path)?;
            WorldInfo {
                edition: "bedrock".into(),
                level_name: summary.level_name,
                version: summary.version_label,
                ..WorldInfo::default()
            }
        }
    };
    if info.level_name.is_empty() {
        info.level_name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    }
    // Keep the existing wire field for compatibility; its payload is now the
    // original world icon, not the pixels of an in-game map item.
    info.map_preview = world_icon(path, edition);
    Ok(info)
}

// Normalize the original icon once per detection, never during page rendering.
// The 64x64 raster stays well below the host's inline-image budget even when
// the original Bedrock screenshot is a large JPEG.
fn world_icon(path: &Path, edition: Edition) -> String {
    worldport_core::world_icon::preview(path, edition)
        .map(|png| format!("data:image/png;base64,{}", worldport_core::map_item::preview::base64(&png)))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use worldport_core::nbt::{compound, tag_string};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(name: &str) -> Self {
            let id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir()
                .join(format!("worldport-info-{name}-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn bedrock_detection_does_not_open_leveldb() {
        let root = Fixture::new("bedrock");
        // The directory deliberately has no LevelDB MANIFEST/CURRENT/SST.
        std::fs::create_dir(root.0.join("db")).unwrap();
        let mut nbt = vec![10, 0, 0, 8];
        fn string(bytes: &mut Vec<u8>, value: &str) {
            bytes.extend_from_slice(&(value.len() as u16).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
        string(&mut nbt, "LevelName");
        string(&mut nbt, "Bedrock fixture");
        nbt.push(9);
        string(&mut nbt, "lastOpenedWithVersion");
        nbt.push(3);
        nbt.extend_from_slice(&4_i32.to_le_bytes());
        for value in [1_i32, 26, 60, 5] {
            nbt.extend_from_slice(&value.to_le_bytes());
        }
        nbt.push(0);
        let mut level = 10_u32.to_le_bytes().to_vec();
        level.extend_from_slice(&(nbt.len() as u32).to_le_bytes());
        level.extend_from_slice(&nbt);
        std::fs::write(root.0.join("level.dat"), level).unwrap();

        // A valid 1x1 RGB PNG. Detection decodes actual image data, not only
        // its signature, and normalizes it into the bounded preview format.
        let png = [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82,
            0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 144, 119, 83, 222,
            0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96, 0,
            0, 0, 4, 0, 1, 246, 23, 56, 85, 0, 0, 0, 0, 73, 69, 78, 68,
            174, 66, 96, 130,
        ];
        std::fs::write(root.0.join("world_icon.png"), png).unwrap();
        let info = world_info(&root.0).expect("metadata without LevelDB");
        assert_eq!(info.level_name, "Bedrock fixture");
        assert_eq!(info.version, "1.26.60.5");
        assert_eq!(info.edition, "bedrock");
        assert!(info.map_preview.starts_with("data:image/png;base64,"));
        assert!(info.map_preview.len() < 128 * 1024);
        assert!(info.dimensions.is_empty());
        assert_eq!(info.total_chunks(), 0);

        // Malformed images produce the normal empty preview.
        std::fs::write(root.0.join("world_icon.png"), b"not a PNG").unwrap();
        assert!(world_info(&root.0).unwrap().map_preview.is_empty());
    }

    #[test]
    fn java_detection_reports_actual_version_without_reading_regions() {
        let root = Fixture::new("java");
        std::fs::create_dir(root.0.join("region")).unwrap();
        // Intentionally unreadable as Anvil data; metadata inspection must
        // never attempt to read a region header.
        std::fs::write(root.0.join("region/r.0.0.mca"), b"not a region").unwrap();
        let level = compound([("Data", compound([
            ("LevelName", tag_string("Java fixture")),
            ("Version", compound([("Name", tag_string("1.21.7"))])),
        ]))]);
        worldport_core::je::nbt::write_gzip(&root.0.join("level.dat"), "", &level).unwrap();
        let info = world_info(&root.0).unwrap();
        assert_eq!(info.level_name, "Java fixture");
        assert_eq!(info.version, "1.21.7");
        assert_eq!(info.edition, "java");
        assert!(info.dimensions.is_empty());
        assert!(info.map_preview.is_empty());
    }
}
