//! Java map colour palette and Bedrock RGBA conversion.
//!
//! Java maps store one byte per pixel: `base_index * 4 + shade`. Bedrock stores
//! raw RGBA. The base palette and shade modifiers match vanilla and are taken
//! from the MIT-licensed Chunker `JavaMapColorsResolver`.

/// Java base colours as `(r, g, b, a)`.
pub const MAP_BASE_COLORS: &[[u8; 4]] = &[
    [0, 0, 0, 0],
    [127, 178, 56, 255],
    [247, 233, 163, 255],
    [199, 199, 199, 255],
    [255, 0, 0, 255],
    [160, 160, 255, 255],
    [167, 167, 167, 255],
    [0, 124, 0, 255],
    [255, 255, 255, 255],
    [164, 168, 184, 255],
    [151, 109, 77, 255],
    [112, 112, 112, 255],
    [64, 64, 255, 255],
    [143, 119, 72, 255],
    [255, 252, 245, 255],
    [216, 127, 51, 255],
    [178, 76, 216, 255],
    [102, 153, 216, 255],
    [229, 229, 51, 255],
    [127, 204, 25, 255],
    [242, 127, 165, 255],
    [76, 76, 76, 255],
    [153, 153, 153, 255],
    [76, 127, 153, 255],
    [127, 63, 178, 255],
    [51, 76, 178, 255],
    [102, 76, 51, 255],
    [102, 127, 51, 255],
    [153, 51, 51, 255],
    [25, 25, 25, 255],
    [250, 238, 77, 255],
    [92, 219, 213, 255],
    [74, 128, 255, 255],
    [0, 217, 58, 255],
    [129, 86, 49, 255],
    [112, 2, 0, 255],
    [209, 177, 161, 255],
    [159, 82, 36, 255],
    [149, 87, 108, 255],
    [112, 108, 138, 255],
    [186, 133, 36, 255],
    [103, 117, 53, 255],
    [160, 77, 78, 255],
    [57, 41, 35, 255],
    [135, 107, 98, 255],
    [87, 92, 92, 255],
    [122, 73, 88, 255],
    [76, 62, 92, 255],
    [76, 50, 35, 255],
    [76, 82, 42, 255],
    [142, 60, 46, 255],
    [37, 22, 16, 255],
    [189, 48, 49, 255],
    [148, 63, 97, 255],
    [92, 25, 29, 255],
    [22, 126, 134, 255],
    [58, 142, 140, 255],
    [86, 44, 62, 255],
    [20, 180, 133, 255],
    [100, 100, 100, 255],
    [216, 175, 147, 255],
    [127, 167, 150, 255],
];

/// Shade multipliers applied to every base colour, in index order.
pub const MAP_SHADES: [u16; 4] = [180, 220, 255, 135];

/// Expands the palette to the 4-shade table used by Java map bytes.
#[must_use]
pub fn shaded_palette() -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(MAP_BASE_COLORS.len() * MAP_SHADES.len());
    for base in MAP_BASE_COLORS {
        for shade in MAP_SHADES {
            let scale = |value: u8| ((u16::from(value) * shade) / 255) as u8;
            out.push([scale(base[0]), scale(base[1]), scale(base[2]), base[3]]);
        }
    }
    out
}

/// RGBA for a Java map byte.
#[must_use]
pub fn java_index_to_rgba(index: u8, palette: &[[u8; 4]]) -> [u8; 4] {
    palette.get(usize::from(index)).copied().unwrap_or([0, 0, 0, 0])
}

/// Nearest Java map byte for an RGBA value (Euclidean distance in RGBA).
#[must_use]
pub fn rgba_to_java_index(rgba: [u8; 4], palette: &[[u8; 4]]) -> u8 {
    let mut best = 0_u8;
    let mut best_score = u32::MAX;
    for (index, entry) in palette.iter().enumerate() {
        let mut score = 0_u32;
        for channel in 0..4 {
            let delta = i32::from(entry[channel]) - i32::from(rgba[channel]);
            score += (delta * delta) as u32;
        }
        if score < best_score {
            best_score = score;
            best = u8::try_from(index).unwrap_or(0);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_round_trips_exact_colours() {
        let palette = shaded_palette();
        for index in 0..palette.len() {
            let rgba = palette[index];
            let found = rgba_to_java_index(rgba, &palette);
            assert_eq!(palette[usize::from(found)], rgba, "index {index}");
        }
    }

    #[test]
    fn transparent_maps_to_zero() {
        let palette = shaded_palette();
        assert_eq!(rgba_to_java_index([0, 0, 0, 0], &palette), 0);
    }
}
