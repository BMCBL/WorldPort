//! Renders a map item to a PNG the plugin page can show.
//!
//! The plugin runs in a sandbox with no filesystem access, so a preview has to
//! arrive as data. The page takes an image `src`, so the map is encoded as a PNG
//! and handed over as a `data:` URL.
//!
//! The encoder is deliberately minimal: maps are 128x128 with no alpha and no
//! interlacing, so a full general-purpose PNG crate would be a large dependency
//! for one fixed shape.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::error::Result;
use crate::map_item::{MapData, java_colors_to_rgba};

/// Placeholder drawn when the world has no map at all.
///
/// Inline SVG rather than a packaged file: it has to render in a page that
/// received no preview, and a missing image would look like a bug.
pub const PLACEHOLDER_SVG: &str = "data:image/svg+xml;base64,\
PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMjgiIGhlaWdodD0i\
MTI4IiB2aWV3Qm94PSIwIDAgMTI4IDEyOCI+PHJlY3Qgd2lkdGg9IjEyOCIgaGVpZ2h0PSIxMjgiIGZp\
bGw9IiMyYjJmMzMiLz48cGF0aCBkPSJNMzIgOTZsMjQtMzIgMjQgMjR6IiBmaWxsPSIjNDU0YjUxIi8+\
PGNpcmNsZSBjeD0iODYiIGN5PSI0MiIgcj0iOCIgZmlsbD0iIzQ1NGI1MSIvPjwvc3ZnPg==";

/// Encodes a map as a PNG `data:` URL.
#[must_use]
pub fn preview_data_url(map: &MapData, palette: &[[u8; 4]]) -> String {
    match encode_png(map, palette) {
        Ok(bytes) => format!("data:image/png;base64,{}", base64(&bytes)),
        // A preview is a convenience; a failure here must not break detection.
        Err(_) => PLACEHOLDER_SVG.to_string(),
    }
}

/// Encodes a map as PNG bytes.
fn encode_png(map: &MapData, palette: &[[u8; 4]]) -> Result<Vec<u8>> {
    let width = u32::try_from(map.width.max(0)).unwrap_or(0);
    let height = u32::try_from(map.height.max(0)).unwrap_or(0);
    let rgba = java_colors_to_rgba(&map.colors, palette);

    // PNG scanlines: one filter byte (0 = none) then RGB triples.
    let mut raw = Vec::with_capacity((width as usize * 3 + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0);
        for column in 0..width as usize {
            let offset = (row * width as usize + column) * 4;
            if let Some(pixel) = rgba.get(offset..offset + 4) {
                raw.extend_from_slice(&pixel[..3]);
            }
        }
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&raw)
        .map_err(|error| crate::error::WorldPortError::io(std::path::Path::new("map preview"), error))?;
    let compressed = encoder
        .finish()
        .map_err(|error| crate::error::WorldPortError::io(std::path::Path::new("map preview"), error))?;

    let mut png = Vec::with_capacity(compressed.len() + 128);
    png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB, no interlace
    write_chunk(&mut png, b"IHDR", &header);
    write_chunk(&mut png, b"IDAT", &compressed);
    write_chunk(&mut png, b"IEND", &[]);
    Ok(png)
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.finish().to_be_bytes());
}

/// CRC-32 as PNG requires it (IEEE 802.3 polynomial, reflected).
struct Crc32(u32);

impl Crc32 {
    const fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            let mut value = (self.0 ^ u32::from(*byte)) & 0xFF;
            for _ in 0..8 {
                value = if value & 1 != 0 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            self.0 = (self.0 >> 8) ^ value;
        }
    }

    const fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

/// Standard base64 with padding, which is what a `data:` URL needs.
/// Encodes bytes using standard padded Base64 for inline images.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let a = u32::from(chunk[0]);
        let b = u32::from(*chunk.get(1).unwrap_or(&0));
        let c = u32::from(*chunk.get(2).unwrap_or(&0));
        let triple = (a << 16) | (b << 8) | c;
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}
