//! Converts the actual save thumbnail, independently of in-game map items.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use crate::convert::Edition;
use crate::error::{Result, WorldPortError, io_result};
use crate::report::{ReportBuilder, WarningCode};

const MAX_ENCODED_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SIDE: u32 = 4096;

fn source_icon(root: &Path, edition: Edition) -> Option<PathBuf> {
    let names: &[&str] = match edition {
        Edition::Java => &["icon.png"],
        Edition::Bedrock => &["world_icon.jpeg", "world_icon.jpg", "world_icon.png"],
    };
    names.iter().map(|name| root.join(name)).find(|path| path.is_file())
}

fn read(path: &Path) -> Result<DynamicImage> {
    let file = io_result(path, std::fs::File::open(path))?;
    let mut bytes = Vec::new();
    io_result(path, file.take(MAX_ENCODED_BYTES + 1).read_to_end(&mut bytes))?;
    if bytes.len() as u64 > MAX_ENCODED_BYTES {
        return Err(WorldPortError::invalid("world icon exceeds 16 MiB"));
    }
    let format = image::guess_format(&bytes)
        .map_err(|error| WorldPortError::invalid(format!("world icon: {error}")))?;
    if !matches!(format, ImageFormat::Jpeg | ImageFormat::Png) {
        return Err(WorldPortError::unsupported("world icon must be PNG or JPEG"));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|error| WorldPortError::invalid(format!("world icon: {error}")))
}

/// Encodes a small square preview suitable for a plugin image node.
/// Returns no preview when the source world has no usable thumbnail.
pub fn preview(root: &Path, edition: Edition) -> Option<Vec<u8>> {
    let image = read(&source_icon(root, edition)?).ok()?;
    // 6 KiB minus the data URL prefix and Base64 expansion fits even
    // hosts that enforce the older 8 KiB per-string view limit.
    const MAX_PREVIEW_BYTES: usize = 6 * 1024 - 32;
    let thumbnail = image.resize_to_fill(64, 64, FilterType::Triangle);
    let mut output = Cursor::new(Vec::new());
    thumbnail.write_to(&mut output, ImageFormat::Png).ok()?;
    if output.get_ref().len() <= MAX_PREVIEW_BYTES {
        return Some(output.into_inner());
    }
    // Preserve the 64x64 dimensions for noisy screenshots; JPEG is only
    // used for the UI preview, never for converting the original icon.
    let rgb = thumbnail.to_rgb8();
    for quality in [90, 75, 50] {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&rgb).ok()?;
        if bytes.len() <= MAX_PREVIEW_BYTES {
            return Some(bytes);
        }
    }
    None
}

fn convert(source: &Path, target: &Path, edition: Edition) -> Result<()> {
    let Some(path) = source_icon(source, edition) else { return Ok(()) };
    let image = read(&path)?;
    match edition {
        Edition::Bedrock => {
            // Java's world selection expects a square 64x64 PNG.
            let icon = image.resize_to_fill(64, 64, FilterType::Lanczos3);
            icon.save_with_format(target.join("icon.png"), ImageFormat::Png)
                .map_err(|error| WorldPortError::other(format!("write Java world icon: {error}")))?;
        }
        Edition::Java => {
            // Bedrock uses a JPEG thumbnail. Preserve the square source icon
            // without introducing an artificial aspect ratio.
            let icon = if image.width() > 256 || image.height() > 256 {
                image.thumbnail(256, 256).to_rgb8()
            } else {
                image.into_rgb8()
            };
            let path = target.join("world_icon.jpeg");
            let file = io_result(&path, std::fs::File::create(&path))?;
            image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90)
                .encode_image(&icon)
                .map_err(|error| WorldPortError::other(format!("write Bedrock world icon: {error}")))?;
        }
    }
    Ok(())
}

/// Converts a save icon if present; a corrupt optional image never aborts terrain conversion.
pub(crate) fn transfer(source: &Path, target: &Path, edition: Edition, report: &mut ReportBuilder) {
    if let Err(error) = convert(source, target, edition) {
        report.warn(WarningCode::Skipped, "world-icon", error.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_thumbnails_convert_to_real_target_formats() {
        let source = tempfile::tempdir().unwrap();
        let java = tempfile::tempdir().unwrap();
        let bedrock = tempfile::tempdir().unwrap();
        image::RgbImage::from_pixel(320, 180, image::Rgb([30, 150, 70]))
            .save_with_format(source.path().join("world_icon.jpeg"), ImageFormat::Jpeg).unwrap();

        convert(source.path(), java.path(), Edition::Bedrock).unwrap();
        let png = std::fs::read(java.path().join("icon.png")).unwrap();
        assert_eq!(image::guess_format(&png).unwrap(), ImageFormat::Png);
        let icon = image::load_from_memory(&png).unwrap();
        assert_eq!((icon.width(), icon.height()), (64, 64));

        convert(java.path(), bedrock.path(), Edition::Java).unwrap();
        let jpeg = std::fs::read(bedrock.path().join("world_icon.jpeg")).unwrap();
        assert_eq!(image::guess_format(&jpeg).unwrap(), ImageFormat::Jpeg);
        let icon = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((icon.width(), icon.height()), (64, 64));

        let preview = preview(source.path(), Edition::Bedrock).unwrap();
        assert!(preview.len() < 90 * 1024);
        assert!(preview.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn high_entropy_preview_fits_legacy_view_string_budget() {
        let source = tempfile::tempdir().unwrap();
        let mut seed = 1_u32;
        let icon = image::RgbImage::from_fn(64, 64, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            image::Rgb([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8])
        });
        icon.save(source.path().join("icon.png")).unwrap();
        let bytes = preview(source.path(), Edition::Java).unwrap();
        assert!(22 + bytes.len().div_ceil(3) * 4 <= 8192);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (64, 64));
    }

    #[test]
    fn missing_and_corrupt_icons_do_not_abort_conversion() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let mut report = ReportBuilder::new("", "", "", "");
        transfer(source.path(), target.path(), Edition::Bedrock, &mut report);
        assert!(!target.path().join("icon.png").exists());
        std::fs::write(source.path().join("world_icon.jpeg"), b"not a jpeg").unwrap();
        transfer(source.path(), target.path(), Edition::Bedrock, &mut report);
        assert!(!target.path().join("icon.png").exists());
    }
}
