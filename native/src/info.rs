//! Lightweight world metadata and original save icons for the plugin.

use std::io::Read;
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

// The host validates PNG/JPEG dimensions before decoding. Bound the encoded
// payload below its per-image view budget (128 KiB) without scanning the save.
const MAX_ICON_BYTES: usize = 90 * 1024;

fn world_icon(path: &Path, edition: Edition) -> String {
    let filenames: &[&str] = match edition {
        Edition::Java => &["icon.png"],
        Edition::Bedrock => &["world_icon.jpeg", "world_icon.jpg", "world_icon.png"],
    };
    for name in filenames {
        let Ok(file) = std::fs::File::open(path.join(name)) else { continue };
        let mut bytes = Vec::new();
        if file.take((MAX_ICON_BYTES + 1) as u64).read_to_end(&mut bytes).is_err()
            || bytes.len() > MAX_ICON_BYTES
        {
            continue;
        }
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else {
            continue;
        };
        return format!("data:{mime};base64,{}", worldport_core::map_item::preview::base64(&bytes));
    }
    String::new()
}
