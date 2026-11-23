use std::collections::{BTreeMap, BTreeSet};

use bedrock_nbt::NbtTag;

use crate::nbt::{NbtExt, TagValue};

use super::{StateDiff, render_state};

const AIR: &str = "minecraft:air";
const INVALID: &str = "<invalid palette index>";

#[derive(Clone, Copy)]
pub(super) enum Cells {
    Blocks,
    Biomes,
}

impl Cells {
    fn format(self) -> (&'static str, usize, u32, &'static str) {
        match self {
            Self::Blocks => ("block_states", 4096, 4, AIR),
            Self::Biomes => ("biomes", 64, 1, "<missing biome>"),
        }
    }

    fn label(self, tag: &NbtTag) -> String {
        match self {
            Self::Blocks => render_state(tag),
            Self::Biomes => tag.as_str_value().unwrap_or(INVALID).to_owned(),
        }
    }
}

struct Palette<'a> {
    names: Vec<String>,
    data: &'a [i64],
    bits: u32,
    per_word: usize,
}

impl<'a> Palette<'a> {
    fn from_section(section: Option<&'a NbtTag>, kind: Cells, key: &str, min_bits: u32) -> Self {
        let packed = section.and_then(|section| section.get(key));
        let names: Vec<_> = packed
            .and_then(|packed| packed.get_list("palette"))
            .unwrap_or_default()
            .iter()
            .map(|entry| kind.label(entry))
            .collect();
        let data = packed
            .and_then(|packed| packed.get_i64_array("data"))
            .unwrap_or_default();
        let bits = (usize::BITS - names.len().saturating_sub(1).leading_zeros()).max(min_bits);
        Self {
            names,
            data,
            bits,
            per_word: (64 / bits) as usize,
        }
    }

    fn index(&self, cell: usize) -> usize {
        if self.names.len() <= 1 {
            return 0;
        }
        let Some(word) = self.data.get(cell / self.per_word) else {
            return usize::MAX;
        };
        let shift = (cell % self.per_word) as u32 * self.bits;
        ((*word as u64 >> shift) & ((1_u64 << self.bits) - 1)) as usize
    }

    fn label(&self, index: usize, empty: &'static str) -> &str {
        if self.names.is_empty() {
            return empty;
        }
        self.names.get(index).map_or(INVALID, String::as_str)
    }
}

#[derive(Default)]
pub(super) struct Differences {
    pub mismatches: u64,
    pub occupied: u64,
    pub first_states: BTreeSet<String>,
    pub rows: Vec<StateDiff>,
}

pub(super) fn compare(first: &NbtTag, second: &NbtTag, kind: Cells) -> Differences {
    let first = sections(first);
    let second = sections(second);
    let positions: BTreeSet<_> = first.keys().chain(second.keys()).copied().collect();
    let mut result = Differences::default();
    let mut rows = BTreeMap::new();
    let (key, count, min_bits, empty) = kind.format();
    for y in positions {
        let left = Palette::from_section(first.get(&y).copied(), kind, key, min_bits);
        let right = Palette::from_section(second.get(&y).copied(), kind, key, min_bits);
        compare_section(&left, &right, count, empty, &mut result, &mut rows);
    }
    result.rows = rows
        .into_iter()
        .map(|(state, (first, second))| StateDiff {
            state,
            first,
            second,
        })
        .collect();
    result
}

fn compare_section(
    left: &Palette<'_>,
    right: &Palette<'_>,
    count: usize,
    empty: &'static str,
    result: &mut Differences,
    rows: &mut BTreeMap<String, (u64, u64)>,
) {
    let mut used = vec![false; left.names.len()];
    let uniform = left.names.len() <= 1 && right.names.len() <= 1;
    let cells = if uniform { 1 } else { count };
    let weight = if uniform { count as u64 } else { 1 };
    for cell in 0..cells {
        let index = left.index(cell);
        if let Some(used) = used.get_mut(index) {
            *used = true;
        }
        let before = left.label(index, empty);
        let after = right.label(right.index(cell), empty);
        if before != empty || after != empty {
            result.occupied += weight;
        }
        if before != after || before == INVALID {
            result.mismatches += weight;
            rows.entry(before.to_owned()).or_default().0 += weight;
            rows.entry(after.to_owned()).or_default().1 += weight;
        }
    }
    if matches!(empty, AIR) {
        result.first_states.extend(
            left.names
                .iter()
                .zip(used)
                .filter(|(name, used)| *used && *name != AIR)
                .map(|(name, _)| name.clone()),
        );
    }
}

fn sections(chunk: &NbtTag) -> BTreeMap<i64, &NbtTag> {
    chunk
        .get_list("sections")
        .unwrap_or_default()
        .iter()
        .filter_map(|section| Some((section.get_int("Y")?, section)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{compound, tag_list, tag_string};

    fn chunk(block_word: i64, biome_word: i64) -> NbtTag {
        compound([(
            "sections",
            tag_list(vec![compound([
                ("Y", NbtTag::Byte(-4)),
                (
                    "block_states",
                    compound([
                        (
                            "palette",
                            tag_list(vec![
                                compound([("Name", tag_string(AIR))]),
                                compound([("Name", tag_string("minecraft:stone"))]),
                            ]),
                        ),
                        (
                            "data",
                            NbtTag::LongArray([vec![block_word], vec![0; 255]].concat()),
                        ),
                    ]),
                ),
                (
                    "biomes",
                    compound([
                        (
                            "palette",
                            tag_list(vec![
                                tag_string("minecraft:plains"),
                                tag_string("minecraft:desert"),
                            ]),
                        ),
                        ("data", NbtTag::LongArray(vec![biome_word])),
                    ]),
                ),
            ])]),
        )])
    }

    #[test]
    fn detects_positions_swapped_with_equal_histograms() {
        let first = chunk(1, 1);
        let second = chunk(16, 2);
        let blocks = compare(&first, &second, Cells::Blocks);
        assert_eq!(blocks.occupied, 2);
        assert_eq!(blocks.mismatches, 2);
        assert_eq!(
            blocks.first_states,
            BTreeSet::from(["minecraft:stone".to_owned()])
        );
        assert_eq!(compare(&first, &second, Cells::Biomes).mismatches, 2);
        assert_eq!(compare(&first, &first, Cells::Blocks).mismatches, 0);
    }
}
