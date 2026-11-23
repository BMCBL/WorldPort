//! Neighbour-dependent Java block states.
//!
//! Bedrock stores far less about a block's surroundings than Java does. A fence,
//! a glass pane or a set of iron bars carries no connection properties at all
//! there - the client derives them - stairs keep a direction but not Java's
//! `shape`, and a chest keeps a facing but not `type`. A per-block mapping
//! therefore cannot reproduce them: they have to be recomputed from the blocks
//! around each position.
//!
//! This module works on one section at a time. That covers every horizontal
//! neighbour, because a section is a full 16x16 footprint; the only positions it
//! cannot answer are the ones whose neighbour lies in the adjacent chunk. Those
//! keep the plain default rather than a guess.
//!
//! Only connections between blocks of the same family are derived here. Java also
//! connects a fence or a pane to any sturdy face, which needs a full-cube table
//! this crate does not carry yet; approximating it would attach fences to signs
//! and torches, which is worse than leaving them unattached.

use std::collections::BTreeMap;

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::nbt::NbtExt;

/// One section's block grid: the Java palette plus one index per cell.
pub struct SectionGrid<'a> {
    /// Palette entries; index 0 is air.
    pub palette: &'a mut Vec<NbtTag>,
    /// Indexed as `(local_y << 8) | (z << 4) | x`, matching the writer.
    pub indices: &'a mut [u16],
}

/// What a position holds, as far as connection rules care.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Class {
    Air,
    Fence,
    FenceGate,
    Pane,
    Wall,
    Stairs,
    Chest,
    Other,
}

impl Class {
    fn of(name: &str) -> Self {
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        if matches!(short, "air" | "cave_air" | "void_air") {
            return Self::Air;
        }
        if short.ends_with("_fence") {
            return Self::Fence;
        }
        if short.ends_with("_fence_gate") {
            return Self::FenceGate;
        }
        if short.ends_with("_wall") {
            return Self::Wall;
        }
        if short.ends_with("_stairs") {
            return Self::Stairs;
        }
        if short == "glass_pane"
            || short.ends_with("_glass_pane")
            || short == "iron_bars"
            || short.ends_with("_bars")
        {
            return Self::Pane;
        }
        if matches!(short, "chest" | "trapped_chest") {
            return Self::Chest;
        }
        Self::Other
    }

    fn accepts_fence(self) -> bool {
        matches!(self, Self::Fence | Self::FenceGate)
    }

    fn accepts_pane(self) -> bool {
        matches!(self, Self::Pane)
    }

    fn accepts_wall(self) -> bool {
        matches!(self, Self::Wall)
    }
}

/// Horizontal directions with the property name each one drives.
const DIRECTIONS: [(isize, isize, &str); 4] = [
    (0, -1, "north"),
    (1, 0, "east"),
    (0, 1, "south"),
    (-1, 0, "west"),
];

/// A palette entry reduced to what the rules need.
#[derive(Clone, Copy)]
struct Cell {
    class: Class,
    facing: Option<&'static str>,
}

/// Rewrites neighbour-dependent states in one section.
///
/// Returns the number of cells whose state changed, so the caller can tell
/// whether the pass did anything at all.
pub fn apply(grid: SectionGrid<'_>) -> usize {
    let SectionGrid { palette, indices } = grid;
    if palette.len() <= 1 {
        return 0;
    }
    let resolved: Vec<Cell> = palette
        .iter()
        .map(|entry| {
            let name = entry.get_str("Name").unwrap_or("minecraft:air");
            Cell {
                class: Class::of(name),
                facing: entry
                    .get("Properties")
                    .and_then(|properties| properties.get_str("facing"))
                    .and_then(intern_facing),
            }
        })
        .collect();
    // Nothing to do when the section holds none of the affected families.
    if !resolved.iter().any(|cell| {
        matches!(
            cell.class,
            Class::Fence | Class::Pane | Class::Stairs | Class::Chest
        )
    }) {
        return 0;
    }

    // Read every position against the *original* grid, then apply: appending a
    // derived palette entry while the scan is running would make a neighbour that
    // already got its own entry look like "no block", so a row of fences would be
    // derived correctly at one end and disconnected at the other.
    //
    // Reading the original state is also what the rules want: the only properties
    // consulted are a block's class and its facing, neither of which this pass
    // changes.
    let grid_view = Grid {
        indices,
        resolved: &resolved,
    };
    let mut updates: Vec<(usize, NbtTag)> = Vec::new();
    for x in 0..16_usize {
        for z in 0..16_usize {
            for y in 0..16_usize {
                let slot = (y << 8) | (z << 4) | x;
                let index = usize::from(indices[slot]);
                let Some(cell) = resolved.get(index).copied() else {
                    continue;
                };
                let Some(derived) = derive(cell.class, x, z, y, &grid_view) else {
                    continue;
                };
                if derived.is_empty() {
                    continue;
                }
                // Clone the block's own state and overwrite only the derived
                // properties: everything else (half, waterlogged, its facing)
                // must survive untouched.
                let Some(mut entry) = palette
                    .get(index)
                    .and_then(|entry| entry.as_compound())
                    .cloned()
                else {
                    continue;
                };
                let mut properties = match entry.remove("Properties") {
                    Some(NbtTag::Compound(properties)) => properties,
                    _ => IndexMap::new(),
                };
                for (key, value) in derived {
                    properties.insert(key, value);
                }
                entry.insert("Properties".to_string(), NbtTag::Compound(properties));
                updates.push((slot, NbtTag::Compound(entry)));
            }
        }
    }

    let mut changed = 0_usize;
    for (slot, wanted) in updates {
        let Some(slot_index) = find_or_insert(palette, &wanted) else {
            continue;
        };
        if indices[slot] != slot_index {
            indices[slot] = slot_index;
            changed += 1;
        }
    }
    changed
}

struct Grid<'a> {
    indices: &'a [u16],
    resolved: &'a [Cell],
}

impl Grid<'_> {
    fn cell(&self, x: isize, z: isize, y: usize) -> Option<Cell> {
        if !(0..16).contains(&x) || !(0..16).contains(&z) || y >= 16 {
            return None;
        }
        let slot = (y << 8) | ((z as usize) << 4) | x as usize;
        let index = usize::from(*self.indices.get(slot)?);
        self.resolved.get(index).copied()
    }

    fn stair_facing(&self, x: isize, z: isize, y: usize) -> Option<&'static str> {
        let cell = self.cell(x, z, y)?;
        if cell.class == Class::Stairs {
            cell.facing
        } else {
            None
        }
    }

    fn chest_facing(&self, x: isize, z: isize, y: usize) -> Option<&'static str> {
        let cell = self.cell(x, z, y)?;
        if cell.class == Class::Chest {
            cell.facing
        } else {
            None
        }
    }
}

/// Properties a block should have, or `None` when it is not neighbour-dependent
/// or a neighbour lies outside the section.
fn derive(
    class: Class,
    x: usize,
    z: usize,
    y: usize,
    grid: &Grid<'_>,
) -> Option<BTreeMap<String, NbtTag>> {
    // `None` means the neighbour is outside this section: that side keeps the
    // state the plain mapping produced, so an interior block is still corrected
    // instead of the whole ring being skipped.
    let connects = |dx: isize, dz: isize, want: fn(Class) -> bool| -> Option<bool> {
        let nx = x as isize + dx;
        let nz = z as isize + dz;
        if !(0..16).contains(&nx) || !(0..16).contains(&nz) {
            return None;
        }
        Some(grid.cell(nx, nz, y).is_some_and(|cell| want(cell.class)))
    };

    let mut properties = BTreeMap::new();
    match class {
        Class::Fence => {
            for (dx, dz, key) in DIRECTIONS {
                if let Some(joins) = connects(dx, dz, Class::accepts_fence) {
                    properties.insert(key.to_string(), bool_tag(joins));
                }
            }
        }
        Class::Pane => {
            for (dx, dz, key) in DIRECTIONS {
                if let Some(joins) = connects(dx, dz, Class::accepts_pane) {
                    properties.insert(key.to_string(), bool_tag(joins));
                }
            }
        }
        // Stairs are deliberately not derived yet. Bedrock's schema for
        // 1.26.60 carries no corner state, so `shape` has to be computed, but
        // against the real world this rule *increased* block mismatches by about
        // 1,000, which means the mapping between Bedrock's `weirdo_direction` and
        // Java's `facing` does not line up the way this rule assumes. Shipping a
        // guess is worse than leaving `straight`, so it waits for a ground-truth
        // fixture rather than a plausible-looking implementation.
        Class::Stairs => return None,
        Class::Chest => {
            let facing = grid.chest_facing(x as isize, z as isize, y)?;
            properties.insert(
                "type".to_string(),
                NbtTag::String(chest_type(facing, x, z, y, grid).to_string()),
            );
        }
        // Walls keep their own connections: Bedrock stores them, so re-deriving
        // them from adjacency would overwrite `tall` with `low` and lose data.
        Class::Air | Class::Other | Class::FenceGate | Class::Wall => return None,
    }
    Some(properties)
}

/// Java's `shape`, mirroring vanilla `StairsBlock.getStairsShape`.
///
/// Kept, and deliberately unused: see the `Class::Stairs` arm in [`derive`].
///
/// A stair looks one block further in the direction it ascends. A perpendicular
/// stair there turns it into an outer corner; one on the opposite side turns it
/// into an inner corner; and a stair already filling the diagonal suppresses the
/// corner, because the two would overlap.
#[allow(dead_code)]
fn stair_shape(
    facing: &'static str,
    x: usize,
    z: usize,
    y: usize,
    grid: &Grid<'_>,
) -> Option<&'static str> {
    let (fx, fz) = offset_of(facing);
    let (bx, bz) = (x as isize + fx, z as isize + fz);
    if let Some(behind) = grid.stair_facing(bx, bz, y) {
        if axis_of(behind) != axis_of(facing) && can_take_shape(behind, bx, bz, y, grid) {
            return Some(if behind == counter_clockwise(facing) {
                "outer_left"
            } else {
                "outer_right"
            });
        }
    }
    let (ox, oz) = (x as isize - fx, z as isize - fz);
    if let Some(front) = grid.stair_facing(ox, oz, y) {
        if axis_of(front) != axis_of(facing) && can_take_shape(front, ox, oz, y, grid) {
            return Some(if front == counter_clockwise(facing) {
                "inner_left"
            } else {
                "inner_right"
            });
        }
    }
    Some("straight")
}

/// Vanilla's `canTakeShape`: the diagonal in front of the neighbour must not
/// already hold a stair facing the same way.
#[allow(dead_code)]
fn can_take_shape(
    neighbour_facing: &'static str,
    nx: isize,
    nz: isize,
    y: usize,
    grid: &Grid<'_>,
) -> bool {
    let (dx, dz) = offset_of(neighbour_facing);
    match grid.stair_facing(nx + dx, nz + dz, y) {
        Some(diagonal) => diagonal != neighbour_facing,
        None => true,
    }
}

/// Java's chest pairing: two chests on the same axis as their facing become a
/// left and a right half.
fn chest_type(
    facing: &'static str,
    x: usize,
    z: usize,
    y: usize,
    grid: &Grid<'_>,
) -> &'static str {
    // Vanilla `ChestBlock.getConnectedDirection`: a left half looks clockwise
    // from its facing for its partner, a right half counter-clockwise. The pair
    // therefore always sits across the facing, never along it.
    let partners = [
        (clockwise(facing), "left"),
        (counter_clockwise(facing), "right"),
    ];
    for (direction, half) in partners {
        let (dx, dz) = offset_of(direction);
        if grid.chest_facing(x as isize + dx, z as isize + dz, y) == Some(facing) {
            return half;
        }
    }
    "single"
}

fn clockwise(facing: &'static str) -> &'static str {
    match facing {
        "north" => "east",
        "east" => "south",
        "south" => "west",
        _ => "north",
    }
}

/// Finds an identical palette entry, or appends this one.
fn find_or_insert(palette: &mut Vec<NbtTag>, wanted: &NbtTag) -> Option<u16> {
    if let Some(index) = palette.iter().position(|entry| entry == wanted) {
        return u16::try_from(index).ok();
    }
    let index = u16::try_from(palette.len()).ok()?;
    palette.push(wanted.clone());
    Some(index)
}

fn bool_tag(value: bool) -> NbtTag {
    NbtTag::String((if value { "true" } else { "false" }).to_string())
}

fn intern_facing(value: &str) -> Option<&'static str> {
    Some(match value {
        "north" => "north",
        "east" => "east",
        "south" => "south",
        "west" => "west",
        _ => return None,
    })
}

fn offset_of(facing: &'static str) -> (isize, isize) {
    match facing {
        "north" => (0, -1),
        "south" => (0, 1),
        "east" => (1, 0),
        _ => (-1, 0),
    }
}

fn axis_of(facing: &'static str) -> &'static str {
    if facing == "east" || facing == "west" {
        "x"
    } else {
        "z"
    }
}

fn counter_clockwise(facing: &'static str) -> &'static str {
    match facing {
        "north" => "west",
        "west" => "south",
        "south" => "east",
        _ => "north",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{compound, tag_string};

    /// Builds a grid from `(cell, block name, extra properties)` placements.
    fn grid(blocks: &[(usize, &str, &[(&str, &str)])]) -> (Vec<NbtTag>, Vec<u16>) {
        let mut palette = vec![compound([("Name", tag_string("minecraft:air"))])];
        let mut indices = vec![0_u16; 4096];
        for (cell, name, properties) in blocks {
            let mut entry: Vec<(String, NbtTag)> = vec![("Name".to_string(), tag_string(*name))];
            if !properties.is_empty() {
                entry.push((
                    "Properties".to_string(),
                    NbtTag::Compound(
                        properties
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), tag_string(*value)))
                            .collect(),
                    ),
                ));
            }
            let tag = NbtTag::Compound(entry.into_iter().collect());
            let index = match palette.iter().position(|existing| *existing == tag) {
                Some(index) => index,
                None => {
                    palette.push(tag);
                    palette.len() - 1
                }
            };
            indices[*cell] = u16::try_from(index).expect("palette fits");
        }
        (palette, indices)
    }

    /// Cell index in the same order the writer uses.
    const fn cell(x: usize, z: usize) -> usize {
        (z << 4) | x
    }

    fn property_at(palette: &[NbtTag], indices: &[u16], cell: usize, key: &str) -> Option<String> {
        palette[usize::from(indices[cell])]
            .get("Properties")
            .and_then(|properties| properties.get_str(key))
            .map(str::to_owned)
    }

    #[test]
    fn a_fence_line_connects_only_along_the_line() {
        let (mut palette, mut indices) = grid(&[
            (cell(4, 4), "minecraft:oak_fence", &[]),
            (cell(5, 4), "minecraft:oak_fence", &[]),
            (cell(6, 4), "minecraft:oak_fence", &[]),
        ]);
        apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        let middle = cell(5, 4);
        assert_eq!(property_at(&palette, &indices, middle, "east").as_deref(), Some("true"));
        assert_eq!(property_at(&palette, &indices, middle, "west").as_deref(), Some("true"));
        assert_eq!(property_at(&palette, &indices, middle, "north").as_deref(), Some("false"));
        assert_eq!(property_at(&palette, &indices, middle, "south").as_deref(), Some("false"));
        let end = cell(4, 4);
        assert_eq!(property_at(&palette, &indices, end, "east").as_deref(), Some("true"));
        assert_eq!(property_at(&palette, &indices, end, "west").as_deref(), Some("false"));
    }

    #[test]
    fn a_fence_connects_to_a_gate_but_not_to_stone() {
        let (mut palette, mut indices) = grid(&[
            (cell(4, 4), "minecraft:oak_fence", &[]),
            (cell(5, 4), "minecraft:oak_fence_gate", &[]),
            (cell(3, 4), "minecraft:stone", &[]),
        ]);
        apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        let fence = cell(4, 4);
        assert_eq!(property_at(&palette, &indices, fence, "east").as_deref(), Some("true"));
        // Java would attach the fence to stone, but that needs a full-cube
        // table; until then the connection must stay off rather than be guessed.
        assert_eq!(property_at(&palette, &indices, fence, "west").as_deref(), Some("false"));
    }

    #[test]
    fn panes_do_not_connect_to_fences() {
        let (mut palette, mut indices) = grid(&[
            (cell(4, 4), "minecraft:glass_pane", &[]),
            (cell(5, 4), "minecraft:oak_fence", &[]),
            (cell(3, 4), "minecraft:iron_bars", &[]),
        ]);
        apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        let pane = cell(4, 4);
        assert_eq!(property_at(&palette, &indices, pane, "east").as_deref(), Some("false"));
        assert_eq!(property_at(&palette, &indices, pane, "west").as_deref(), Some("true"));
    }

    #[test]
    fn walls_keep_their_own_connections_instead_of_being_re_derived() {
        // Bedrock stores wall connections accurately and the mapper translates
        // them. Recomputing them from adjacency would overwrite a `tall`
        // connection to a stone wall with `none`, so the pass must leave walls
        // completely untouched even when two of them touch.
        let (mut palette, mut indices) = grid(&[
            (
                cell(4, 4),
                "minecraft:cobblestone_wall",
                &[("north", "tall"), ("east", "low"), ("south", "none"), ("west", "none"), ("up", "true")],
            ),
            (cell(5, 4), "minecraft:mossy_cobblestone_wall", &[]),
        ]);
        let before = palette.clone();
        let changed = apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        assert_eq!(changed, 0);
        assert_eq!(palette, before);
    }

    #[test]
    fn stairs_are_left_untouched_while_the_shape_rule_is_disabled() {
        // Bedrock 1.26.60 carries no corner state, so `shape` cannot survive a
        // mapping. Deriving it is a real option, but the first implementation
        // increased block mismatches by about a thousand on a real world, which
        // means the direction convention it assumed is wrong. Until a
        // ground-truth fixture settles that, stairs keep the mapped default -
        // and this test pins that, so enabling the rule has to delete it.
        let (mut palette, mut indices) = grid(&[
            (cell(3, 1), "minecraft:oak_stairs", &[("facing", "east"), ("half", "bottom")]),
            (cell(4, 1), "minecraft:oak_stairs", &[("facing", "east"), ("half", "bottom")]),
            (cell(4, 2), "minecraft:oak_stairs", &[("facing", "south"), ("half", "bottom")]),
        ]);
        let before = palette.clone();
        let changed = apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        assert_eq!(changed, 0);
        assert_eq!(palette, before);
    }

    #[test]
    fn chests_pair_only_along_their_facing_axis() {
        // Two chests stacked along their facing axis are not a double chest: a
        // pair always sits across the facing, never in front of it.
        let (mut palette, mut indices) = grid(&[
            (cell(3, 5), "minecraft:chest", &[("facing", "north")]),
            (cell(3, 6), "minecraft:chest", &[("facing", "north")]),
        ]);
        apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        assert_eq!(
            property_at(&palette, &indices, cell(3, 5), "type").as_deref(),
            Some("single")
        );

        // Facing north, a pair sits across the facing: along x. Vanilla names the
        // half whose partner is clockwise from its facing the left one, so with
        // north that is the eastward partner.
        let (mut palette, mut indices) = grid(&[
            (cell(5, 1), "minecraft:chest", &[("facing", "north")]),
            (cell(6, 1), "minecraft:chest", &[("facing", "north")]),
        ]);
        apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        assert_eq!(
            property_at(&palette, &indices, cell(5, 1), "type").as_deref(),
            Some("left")
        );
        assert_eq!(
            property_at(&palette, &indices, cell(6, 1), "type").as_deref(),
            Some("right")
        );
    }

    #[test]
    fn unrelated_blocks_are_left_completely_alone() {
        let (mut palette, mut indices) = grid(&[(cell(1, 1), "minecraft:stone", &[])]);
        let before = palette.clone();
        let changed = apply(SectionGrid {
            palette: &mut palette,
            indices: &mut indices,
        });
        assert_eq!(changed, 0);
        assert_eq!(palette, before);
    }

    #[test]
    fn a_section_of_air_is_skipped_without_touching_the_palette() {
        let mut palette = vec![compound([("Name", tag_string("minecraft:air"))])];
        let mut indices = vec![0_u16; 4096];
        assert_eq!(
            apply(SectionGrid {
                palette: &mut palette,
                indices: &mut indices,
            }),
            0
        );
        assert_eq!(palette.len(), 1);
    }
}
