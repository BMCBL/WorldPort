//! Scheduled tick conversion through a unified model.
//!
//! Java stores per-chunk `block_ticks` / `fluid_ticks` lists with a *relative
//! delay* (`t`), a priority (`p`) and the block id. Bedrock stores one
//! `PendingTicks` record per chunk with an *absolute* `time` and a full
//! `blockState` (name + states + storage version).
//!
//! Neither side is copied verbatim. Both are read into [`ScheduledTick`], which
//! keeps the position, the block/fluid identity, the remaining delay and the
//! Java priority, and then written in the target edition's shape. Delays are
//! rebased against the target's `currentTick` on Bedrock and stored directly on
//! Java.
//!
//! Conversion preserves pending events and their ordering as far as the data
//! allows. It cannot make the two schedulers behave identically: Java and
//! Bedrock tick ordering, priorities and fluid spread rules differ, so a
//! converted world may still settle differently after the first load.

use std::collections::BTreeMap;

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::nbt::{NbtExt, TagValue, compound, tag_list, tag_string};

/// What is scheduled.
#[derive(Clone, Debug, PartialEq)]
pub enum TickTarget {
    /// A block update, with the Bedrock state form preserved.
    Block {
        /// Block identifier (`minecraft:oak_planks`).
        name: String,
        /// Bedrock state properties (empty for Java-only reads).
        states: BTreeMap<String, NbtTag>,
    },
    /// A fluid update.
    Fluid {
        /// `water` or `lava`.
        fluid: FluidKind,
    },
}

/// Fluid kind carried by a scheduled tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FluidKind {
    /// Water.
    Water,
    /// Lava.
    Lava,
}

impl FluidKind {
    /// Java block id used in `fluid_ticks`.
    #[must_use]
    pub const fn java_id(self) -> &'static str {
        match self {
            Self::Water => "minecraft:water",
            Self::Lava => "minecraft:lava",
        }
    }

    /// Bedrock flowing block id used in `PendingTicks`.
    #[must_use]
    pub const fn bedrock_id(self) -> &'static str {
        match self {
            Self::Water => "minecraft:flowing_water",
            Self::Lava => "minecraft:flowing_lava",
        }
    }

    /// Classifies a Bedrock block state name as a fluid tick target.
    #[must_use]
    pub fn from_bedrock_name(name: &str) -> Option<Self> {
        match name {
            "minecraft:water" | "minecraft:flowing_water" => Some(Self::Water),
            "minecraft:lava" | "minecraft:flowing_lava" => Some(Self::Lava),
            _ => None,
        }
    }
}

/// One pending scheduled tick, edition independent.
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledTick {
    /// Block position.
    pub position: [i32; 3],
    /// Block or fluid being scheduled.
    pub target: TickTarget,
    /// Remaining delay in ticks, always `>= 0`.
    pub delay: i64,
    /// Java priority (`p`); Bedrock has no equivalent, so it is kept for Java
    /// round trips and dropped when writing Bedrock.
    pub priority: i32,
}

impl ScheduledTick {
    /// Builds a block tick.
    #[must_use]
    pub fn block(
        position: [i32; 3],
        name: impl Into<String>,
        states: BTreeMap<String, NbtTag>,
        delay: i64,
    ) -> Self {
        Self {
            position,
            target: TickTarget::Block {
                name: name.into(),
                states,
            },
            delay: delay.max(0),
            priority: 0,
        }
    }

    /// Builds a fluid tick.
    #[must_use]
    pub fn fluid(position: [i32; 3], fluid: FluidKind, delay: i64) -> Self {
        Self {
            position,
            target: TickTarget::Fluid { fluid },
            delay: delay.max(0),
            priority: 0,
        }
    }
}

/// Outcome of writing one tick queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TickStats {
    /// Entries written to the target edition.
    pub written: usize,
    /// Entries dropped because they cannot be represented (non-vanilla ids).
    pub pruned: usize,
}

/// Reads Java `block_ticks` and `fluid_ticks` from a chunk into the unified model.
#[must_use]
pub fn read_java_chunk(chunk: &NbtTag) -> Vec<ScheduledTick> {
    let mut ticks = Vec::new();
    for entry in chunk.get_list("block_ticks").unwrap_or_default() {
        let Some(name) = entry.get_str("i") else {
            continue;
        };
        // A fluid entry may appear in either list; classify by id.
        let target = if let Some(fluid) = FluidKind::from_bedrock_name(name) {
            TickTarget::Fluid { fluid }
        } else {
            TickTarget::Block {
                name: name.to_string(),
                states: BTreeMap::new(),
            }
        };
        ticks.push(ScheduledTick {
            position: position_of(entry),
            target,
            delay: entry.get_int("t").unwrap_or(0).max(0),
            priority: entry.get_int("p").unwrap_or(0) as i32,
        });
    }
    for entry in chunk.get_list("fluid_ticks").unwrap_or_default() {
        let Some(name) = entry.get_str("i") else {
            continue;
        };
        let fluid = FluidKind::from_bedrock_name(name).unwrap_or(FluidKind::Water);
        ticks.push(ScheduledTick {
            position: position_of(entry),
            target: TickTarget::Fluid { fluid },
            delay: entry.get_int("t").unwrap_or(0).max(0),
            priority: entry.get_int("p").unwrap_or(0) as i32,
        });
    }
    ticks
}

/// Writes the unified model back as Java `(block_ticks, fluid_ticks)` lists.
#[must_use]
pub fn write_java_chunk(ticks: &[ScheduledTick]) -> (NbtTag, NbtTag) {
    let mut block_ticks = Vec::new();
    let mut fluid_ticks = Vec::new();
    for tick in ticks {
        let id = match &tick.target {
            TickTarget::Block { name, .. } => name.clone(),
            TickTarget::Fluid { fluid } => fluid.java_id().to_string(),
        };
        let entry = compound([
            ("i", tag_string(id)),
            ("x", NbtTag::Int(tick.position[0])),
            ("y", NbtTag::Int(tick.position[1])),
            ("z", NbtTag::Int(tick.position[2])),
            (
                "t",
                NbtTag::Int(i32::try_from(tick.delay).unwrap_or(i32::MAX)),
            ),
            ("p", NbtTag::Int(tick.priority)),
        ]);
        match tick.target {
            TickTarget::Fluid { .. } => fluid_ticks.push(entry),
            TickTarget::Block { .. } => block_ticks.push(entry),
        }
    }
    (tag_list(block_ticks), tag_list(fluid_ticks))
}

/// Reads a Bedrock `PendingTicks` record into the unified model.
///
/// Absolute `time` values are rebased onto `currentTick`; `-1` (next tick)
/// becomes a zero delay.
#[must_use]
pub fn read_bedrock_pending(record: &NbtTag) -> (Vec<ScheduledTick>, TickStats) {
    let current_tick = record.get_int("currentTick").unwrap_or(0);
    let mut ticks = Vec::new();
    let mut stats = TickStats::default();
    for entry in record.get_list("tickList").unwrap_or_default() {
        let Some(state) = entry.get("blockState") else {
            stats.pruned += 1;
            continue;
        };
        let Some(name) = state.get_str("name") else {
            stats.pruned += 1;
            continue;
        };
        if !crate::mapping::is_vanilla(name) {
            stats.pruned += 1;
            continue;
        }
        stats.written += 1;
        let target = if let Some(fluid) = FluidKind::from_bedrock_name(name) {
            TickTarget::Fluid { fluid }
        } else {
            TickTarget::Block {
                name: name.to_string(),
                states: state
                    .get("states")
                    .and_then(|states| states.as_compound())
                    .map(|states| {
                        states
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
            }
        };
        let time = entry.get_int("time").unwrap_or(-1);
        let delay = if time < 0 {
            0
        } else {
            time.saturating_sub(current_tick).max(0)
        };
        ticks.push(ScheduledTick {
            position: position_of(entry),
            target,
            delay,
            // Bedrock has no priority; normal priority keeps Java semantics.
            priority: 0,
        });
    }
    (ticks, stats)
}

/// Writes the unified model as a Bedrock `PendingTicks` record.
///
/// `state_of` resolves the converted block state at a position so block ticks
/// carry the full palette entry Bedrock expects; fluid ticks always write the
/// flowing fluid id with depth 0.
#[must_use]
pub fn write_bedrock_pending(
    ticks: &[ScheduledTick],
    current_tick: i64,
    state_version: i32,
    state_of: impl Fn(i32, i32, i32) -> Option<(String, BTreeMap<String, NbtTag>)>,
) -> (Option<NbtTag>, TickStats) {
    let mut list = Vec::new();
    let mut stats = TickStats::default();
    for tick in ticks {
        let [x, y, z] = tick.position;
        let (name, states) = match &tick.target {
            TickTarget::Block { name, states } => state_of(x, y, z).unwrap_or_else(|| {
                (name.clone(), states.clone())
            }),
            TickTarget::Fluid { fluid } => {
                let mut states = BTreeMap::new();
                states.insert("liquid_depth".to_string(), NbtTag::Int(0));
                (fluid.bedrock_id().to_string(), states)
            }
        };
        if !crate::mapping::is_vanilla(&name) {
            stats.pruned += 1;
            continue;
        }
        stats.written += 1;
        let block_state = compound([
            ("name", tag_string(name)),
            (
                "states",
                NbtTag::Compound(states.into_iter().collect::<IndexMap<String, NbtTag>>()),
            ),
            ("version", NbtTag::Int(state_version)),
        ]);
        list.push(compound([
            ("blockState", block_state),
            (
                "time",
                NbtTag::Long(current_tick.saturating_add(tick.delay)),
            ),
            ("x", NbtTag::Int(x)),
            ("y", NbtTag::Int(y)),
            ("z", NbtTag::Int(z)),
        ]));
    }
    if list.is_empty() {
        return (None, stats);
    }
    (
        Some(compound([
            ("currentTick", NbtTag::Long(current_tick)),
            ("tickList", tag_list(list)),
        ])),
        stats,
    )
}

fn position_of(entry: &NbtTag) -> [i32; 3] {
    [
        entry.get_int("x").unwrap_or(0) as i32,
        entry.get_int("y").unwrap_or(0) as i32,
        entry.get_int("z").unwrap_or(0) as i32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn java_chunk() -> NbtTag {
        compound([
            (
                "block_ticks",
                tag_list(vec![compound([
                    ("i", tag_string("minecraft:oak_planks")),
                    ("x", NbtTag::Int(2)),
                    ("y", NbtTag::Int(-62)),
                    ("z", NbtTag::Int(2)),
                    ("t", NbtTag::Int(40)),
                    ("p", NbtTag::Int(1)),
                ])]),
            ),
            (
                "fluid_ticks",
                tag_list(vec![compound([
                    ("i", tag_string("minecraft:water")),
                    ("x", NbtTag::Int(5)),
                    ("y", NbtTag::Int(-63)),
                    ("z", NbtTag::Int(5)),
                    ("t", NbtTag::Int(5)),
                    ("p", NbtTag::Int(0)),
                ])]),
            ),
        ])
    }

    #[test]
    fn unified_model_round_trips_through_both_editions() {
        let ticks = read_java_chunk(&java_chunk());
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].delay, 40);
        assert_eq!(ticks[0].priority, 1);
        assert!(matches!(ticks[1].target, TickTarget::Fluid { fluid: FluidKind::Water }));

        // Java -> Bedrock: absolute time, full block state.
        let (record, stats) = write_bedrock_pending(&ticks, 1000, 18_496_512, |_, _, _| {
            Some(("minecraft:oak_planks".to_string(), BTreeMap::new()))
        });
        let record = record.expect("pending ticks");
        assert_eq!(stats.written, 2);
        assert_eq!(stats.pruned, 0);
        assert_eq!(record.get_int("currentTick"), Some(1000));
        let list = record.get_list("tickList").expect("tick list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].get_int("time"), Some(1040));
        assert_eq!(
            list[1]
                .get("blockState")
                .and_then(|state| state.get_str("name")),
            Some("minecraft:flowing_water")
        );

        // Bedrock -> Java: rebased delays, fluid back to water.
        let (back, stats) = read_bedrock_pending(&record);
        assert_eq!(stats.written, 2);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].delay, 40);
        assert_eq!(back[1].delay, 5);
        let (block, fluid) = write_java_chunk(&back);
        assert_eq!(block.get_list("").map(|values| values.len()), None);
        let block_list = match &block {
            NbtTag::List(values) => values,
            _ => panic!("block_ticks must be a list"),
        };
        let fluid_list = match &fluid {
            NbtTag::List(values) => values,
            _ => panic!("fluid_ticks must be a list"),
        };
        assert_eq!(block_list.len(), 1);
        assert_eq!(fluid_list.len(), 1);
        assert_eq!(block_list[0].get_str("i"), Some("minecraft:oak_planks"));
        assert_eq!(fluid_list[0].get_str("i"), Some("minecraft:water"));
    }

    #[test]
    fn negative_bedrock_time_means_next_tick() {
        let record = compound([
            ("currentTick", NbtTag::Long(500)),
            (
                "tickList",
                tag_list(vec![compound([
                    (
                        "blockState",
                        compound([("name", tag_string("minecraft:flowing_lava"))]),
                    ),
                    ("time", NbtTag::Long(-1)),
                    ("x", NbtTag::Int(0)),
                    ("y", NbtTag::Int(0)),
                    ("z", NbtTag::Int(0)),
                ])]),
            ),
        ]);
        let (ticks, _) = read_bedrock_pending(&record);
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].delay, 0);
        assert!(matches!(
            ticks[0].target,
            TickTarget::Fluid {
                fluid: FluidKind::Lava
            }
        ));
    }

    #[test]
    fn non_vanilla_block_ticks_are_pruned() {
        let ticks = vec![ScheduledTick::block(
            [0, 0, 0],
            "modded:magic_block",
            BTreeMap::new(),
            5,
        )];
        let (record, stats) = write_bedrock_pending(&ticks, 0, 1, |_, _, _| None);
        assert!(record.is_none());
        assert_eq!(stats.pruned, 1);
    }
}
