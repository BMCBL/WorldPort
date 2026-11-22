//! WorldPort command-line interface: inspect, convert and (later) compare worlds.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use worldport_core::be;
use worldport_core::convert::{ConversionOptions, ConversionPlan, Edition, detect_edition};
use worldport_core::je::anvil::RegionFile;
use worldport_core::je::chunk::summarize as summarize_je_chunk;
use worldport_core::je::nbt as je_nbt;
use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::nbt::NbtExt;

#[derive(Debug, Parser)]
#[command(
    name = "worldport",
    version,
    about = "Convert Minecraft worlds between Bedrock and Java with full entity and NBT support"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what a world contains without converting it.
    Inspect(InspectArgs),
    /// Convert a world to the other edition.
    Convert(ConvertArgs),
    /// Decode an NBT file (gzip or raw) and print it as SNBT.
    Nbt(NbtArgs),
    /// Print one decoded chunk as SNBT.
    Chunk(ChunkArgs),
    /// Compare two Java worlds chunk by chunk (round-trip verification).
    Compare(CompareArgs),
}

#[derive(Debug, Args)]
struct CompareArgs {
    /// First world (usually the original).
    first: PathBuf,
    /// Second world (usually the round-tripped conversion).
    second: PathBuf,
    /// Dimension to compare.
    #[arg(long, default_value = "overworld")]
    dimension: String,
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ChunkArgs {
    /// World directory.
    world: PathBuf,
    /// Dimension: overworld, the_nether or the_end.
    #[arg(long, default_value = "overworld")]
    dimension: String,
    /// Chunk coordinates as `x,z`.
    #[arg(long)]
    at: Option<String>,
    /// Scan the dimension and print the first fully generated chunk instead.
    #[arg(long)]
    search: bool,
    /// Print only the top-level keys.
    #[arg(long)]
    keys: bool,
}

#[derive(Debug, Args)]
struct NbtArgs {
    /// File to decode.
    path: PathBuf,
    /// Treat the file as uncompressed NBT.
    #[arg(long)]
    raw: bool,
    /// Treat the file as a Bedrock `level.dat` (8-byte header + little-endian NBT).
    #[arg(long)]
    bedrock: bool,
    /// Optional NTB path inside the file, e.g. `Data.spawn`.
    #[arg(long)]
    tag: Option<String>,
}

#[derive(Debug, Args)]
struct InspectArgs {
    /// World directory (Bedrock world root with `db/`, or Java world with `level.dat`).
    path: PathBuf,
    /// Print JSON instead of text.
    #[arg(long)]
    json: bool,
    /// Decode chunks to count sections, block entities and entities.
    #[arg(long)]
    deep: bool,
    /// Limit how many chunks are decoded with `--deep`.
    #[arg(long, default_value_t = 4096)]
    limit: usize,
    /// Print the top-level `level.dat` keys (debugging aid).
    #[arg(long)]
    keys: bool,
}

#[derive(Debug, Args)]
struct ConvertArgs {
    /// Source world directory.
    source: PathBuf,
    /// Output world directory (must be empty or missing).
    target: PathBuf,
    /// Target edition; defaults to the other edition.
    #[arg(long, value_parser = parse_edition)]
    to: Option<Edition>,
    /// Target version label, e.g. `1.21.4` (Java) or `1.21.60` (Bedrock).
    #[arg(long)]
    target_version: Option<String>,
    /// Worker threads (0 = all cores).
    #[arg(long, default_value_t = 0)]
    jobs: usize,
    /// Deflate level for Java region chunks.
    #[arg(long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(0..=9))]
    compression: u32,
    /// Replace an existing target directory.
    #[arg(long)]
    overwrite: bool,
    /// Validate and report without writing.
    #[arg(long)]
    dry_run: bool,
    /// Write the report as JSON to this path.
    #[arg(long)]
    report: Option<PathBuf>,
}

fn parse_edition(value: &str) -> Result<Edition, String> {
    match value.to_ascii_lowercase().as_str() {
        "bedrock" | "be" => Ok(Edition::Bedrock),
        "java" | "je" => Ok(Edition::Java),
        other => Err(format!("unknown edition {other}; use bedrock or java")),
    }
}

#[derive(Debug, Serialize)]
struct InspectReport {
    edition: String,
    path: String,
    level_name: String,
    seed: i64,
    data_version: Option<i32>,
    bedrock_version: Option<String>,
    layout: Option<String>,
    dimensions: Vec<DimensionReport>,
    players: usize,
    maps: usize,
    record_counts: Vec<(String, usize)>,
    level_dat_keys: Vec<String>,
    deep: Option<DeepReport>,
}

#[derive(Debug, Serialize)]
struct DimensionReport {
    dimension: String,
    chunks: usize,
}

#[derive(Debug, Default, Serialize)]
struct DeepReport {
    chunks: usize,
    sections: u64,
    non_empty_sections: u64,
    block_entities: u64,
    entities: u64,
    min_section_y: i32,
    max_section_y: i32,
    statuses: BTreeMap<String, u64>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect(args) => inspect(args),
        Command::Convert(args) => convert(args),
        Command::Nbt(args) => dump_nbt(args),
        Command::Chunk(args) => dump_chunk(args),
        Command::Compare(args) => compare_worlds(args),
    }
}

fn dump_chunk(args: ChunkArgs) -> Result<()> {
    let world = JavaWorld::open(&args.world)?;
    let dimension = match args.dimension.as_str() {
        "overworld" => JeDimension::Overworld,
        "the_nether" | "nether" => JeDimension::Nether,
        "the_end" | "end" => JeDimension::End,
        other => bail!("unknown dimension {other}"),
    };
    let (x, z) = match (&args.at, args.search) {
        (Some(at), false) => {
            let (x, z) = at.split_once(',').context("--at expects `x,z`")?;
            (x.trim().parse()?, z.trim().parse()?)
        }
        (None, true) => find_full_chunk(&world, dimension)?,
        _ => bail!("pass either --at x,z or --search"),
    };
    let pos = worldport_core::je::JeChunkPos::new(x, z);
    eprintln!("chunk {}", pos.label());
    let chunk = world
        .read_chunk(dimension, pos)?
        .with_context(|| format!("chunk {} not present", pos.label()))?;
    if args.keys {
        for key in chunk.as_compound().map(|entries| entries.keys()).into_iter().flatten() {
            println!("{key}");
        }
        if let Some(sections) = chunk.get_list("sections") {
            let interesting = sections.iter().find(|section| {
                section
                    .get("block_states")
                    .and_then(|states| states.get_list("palette"))
                    .is_some_and(|palette| {
                        palette.len() > 1
                            || palette
                                .first()
                                .is_some_and(|entry| entry.get_str("Name") != Some("minecraft:air"))
                    })
            });
            if let Some(section) = interesting {
                println!("-- first non-empty section --");
                println!("{}", bedrock_nbt::snbt::to_string(section)?);
            }
        }
    } else {
        println!("{}", bedrock_nbt::snbt::to_string(&chunk)?);
    }
    Ok(())
}

fn compare_worlds(args: CompareArgs) -> Result<()> {
    let dimension = match args.dimension.as_str() {
        "overworld" => JeDimension::Overworld,
        "the_nether" | "nether" => JeDimension::Nether,
        "the_end" | "end" => JeDimension::End,
        other => bail!("unknown dimension {other}"),
    };
    let first = JavaWorld::open(&args.first)?;
    let second = JavaWorld::open(&args.second)?;
    let report = worldport_core::compare::compare_java(&first, &second, dimension)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("first        {} ({} chunks)", report.first_name, report.first_chunks);
    println!("second       {} ({} chunks)", report.second_name, report.second_chunks);
    println!("matched      {}", report.matched_chunks);
    println!("only first   {}", report.only_first);
    println!("only second  {}", report.only_second);
    println!("differing    {} chunks", report.differing_chunks);
    println!("block diffs  {}", report.block_mismatches);
    println!("biome diffs  {}", report.biome_mismatches);
    if !report.level_dat.is_empty() {
        println!("level.dat");
        for (field, left, right) in &report.level_dat {
            println!("  {field}: {left} -> {right}");
        }
    }
    if !report.block_totals.is_empty() {
        println!("top block differences");
        for row in report.block_totals.iter().take(15) {
            println!("  {}: {} -> {}", row.state, row.first, row.second);
        }
    }
    if !report.worst_chunks.is_empty() {
        println!("worst chunks");
        for chunk in report.worst_chunks.iter().take(5) {
            println!(
                "  {}: {} block, {} biome",
                chunk.chunk, chunk.block_mismatches, chunk.biome_mismatches
            );
            for row in chunk.blocks.iter().take(3) {
                println!("    {} {} -> {}", row.state, row.first, row.second);
            }
        }
    }
    if report.is_identical() {
        println!("result       identical");
    } else {
        println!("result       differences found");
    }
    Ok(())
}

fn find_full_chunk(world: &JavaWorld, dimension: JeDimension) -> Result<(i32, i32)> {
    for (region_x, region_z, path) in world.region_files(dimension)? {
        let mut region = RegionFile::open(&path, false)?;
        for (local_x, local_z) in region.occupied_slots() {
            let Some(payload) = region.read_chunk(local_x, local_z)? else {
                continue;
            };
            let Ok(decoded) = je_nbt::decode(&payload) else {
                continue;
            };
            let status = decoded.tag.get_str("Status").unwrap_or_default();
            if status.ends_with("full") {
                return Ok((
                    region_x * 32 + i32::from(local_x),
                    region_z * 32 + i32::from(local_z),
                ));
            }
        }
    }
    bail!("no fully generated chunk found")
}

fn dump_nbt(args: NbtArgs) -> Result<()> {
    if args.bedrock {
        let document = bedrock_world::level::read(&args.path)?;
        let mut tag = &document.root;
        if let Some(path) = &args.tag {
            for segment in path.split('.') {
                tag = tag.get(segment).with_context(|| format!("missing tag {path}"))?;
            }
        }
        println!("{}", bedrock_nbt::snbt::to_string(tag)?);
        return Ok(());
    }
    let named = if args.raw {
        je_nbt::decode(&std::fs::read(&args.path)?)?
    } else {
        je_nbt::read_gzip(&args.path)?
    };
    let mut tag = &named.tag;
    if let Some(path) = &args.tag {
        for segment in path.split('.') {
            tag = tag
                .get(segment)
                .with_context(|| format!("missing tag {path}"))?;
        }
    }
    println!("{}", bedrock_nbt::snbt::to_string(tag)?);
    Ok(())
}

fn inspect(args: InspectArgs) -> Result<()> {
    let edition = detect_edition(&args.path)?;
    let report = match edition {
        Edition::Java => inspect_java(&args)?,
        Edition::Bedrock => inspect_bedrock(&args)?,
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(())
}

fn inspect_java(args: &InspectArgs) -> Result<InspectReport> {
    let world = JavaWorld::open(&args.path)?;
    let mut dimensions = Vec::new();
    let mut deep = DeepReport::default();
    let mut deep_budget = args.limit;
    for (dimension, label) in [
        (JeDimension::Overworld, "overworld"),
        (JeDimension::Nether, "the_nether"),
        (JeDimension::End, "the_end"),
    ] {
        let positions = world.chunk_positions(dimension)?;
        dimensions.push(DimensionReport {
            dimension: label.to_string(),
            chunks: positions.len(),
        });
        if args.deep {
            scan_java_chunks(&world, dimension, deep_budget, &mut deep)?;
            deep_budget = deep_budget.saturating_sub(positions.len());
        }
    }
    let players = world.player_files()?.len();
    let maps = world.map_files()?.len();
    let level_dat_keys = if args.keys {
        world
            .data()
            .as_compound()
            .map(|entries| entries.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(InspectReport {
        edition: "java".to_string(),
        path: world.root().display().to_string(),
        level_name: world.level_name(),
        seed: world.seed(),
        data_version: Some(world.data_version()),
        bedrock_version: None,
        layout: Some(match world.layout() {
            worldport_core::je::WorldLayout::Modern => "modern (dimensions/)".to_string(),
            worldport_core::je::WorldLayout::Legacy => "legacy (region/, DIM-1, DIM1)".to_string(),
        }),
        dimensions,
        players,
        maps,
        record_counts: Vec::new(),
        level_dat_keys,
        deep: args.deep.then_some(deep),
    })
}

fn scan_java_chunks(
    world: &JavaWorld,
    dimension: JeDimension,
    budget: usize,
    deep: &mut DeepReport,
) -> Result<()> {
    if budget == 0 {
        return Ok(());
    }
    for (region_x, region_z, path) in world.region_files(dimension)? {
        let mut region = RegionFile::open(&path, false)
            .with_context(|| format!("open {}", path.display()))?;
        for (local_x, local_z) in region.occupied_slots() {
            if deep.chunks >= budget {
                return Ok(());
            }
            let Some(payload) = region.read_chunk(local_x, local_z)? else {
                continue;
            };
            let Ok(decoded) = je_nbt::decode(&payload) else {
                continue;
            };
            let summary = summarize_je_chunk(&decoded.tag);
            let _ = (region_x, region_z);
            deep.chunks += 1;
            deep.sections += u64::from(summary.sections);
            deep.non_empty_sections += u64::from(summary.non_empty_sections);
            deep.block_entities += u64::from(summary.block_entities);
            deep.entities += u64::from(summary.entities);
            let low = i32::from(summary.min_section_y);
            let high = i32::from(summary.max_section_y);
            if deep.chunks == 1 {
                deep.min_section_y = low;
                deep.max_section_y = high;
            } else {
                deep.min_section_y = deep.min_section_y.min(low);
                deep.max_section_y = deep.max_section_y.max(high);
            }
            *deep.statuses.entry(summary.status).or_insert(0) += 1;
        }
    }
    Ok(())
}

fn inspect_bedrock(args: &InspectArgs) -> Result<InspectReport> {
    let summary = be::inspect::summarize(&args.path)?;
    Ok(InspectReport {
        edition: "bedrock".to_string(),
        path: args.path.display().to_string(),
        level_name: summary.level_name,
        seed: summary.seed,
        data_version: None,
        bedrock_version: Some(summary.version_label),
        layout: Some(format!("storage version {}", summary.storage_version)),
        dimensions: vec![
            DimensionReport {
                dimension: "overworld".to_string(),
                chunks: summary.overworld_chunks,
            },
            DimensionReport {
                dimension: "the_nether".to_string(),
                chunks: summary.nether_chunks,
            },
            DimensionReport {
                dimension: "the_end".to_string(),
                chunks: summary.end_chunks,
            },
        ],
        players: summary.players,
        maps: 0,
        record_counts: summary.record_counts,
        level_dat_keys: Vec::new(),
        deep: None,
    })
}

fn print_text(report: &InspectReport) {
    println!("edition      {}", report.edition);
    println!("path         {}", report.path);
    println!("level name   {}", report.level_name);
    println!("seed         {}", report.seed);
    if let Some(version) = &report.bedrock_version
        && !version.is_empty()
    {
        println!("version      {version} (Bedrock)");
    }
    if let Some(data_version) = report.data_version {
        println!("data version {data_version} (Java)");
    }
    if let Some(layout) = &report.layout {
        println!("layout       {layout}");
    }
    println!("players      {}", report.players);
    println!("maps         {}", report.maps);
    if !report.level_dat_keys.is_empty() {
        println!("level.dat keys");
        for key in &report.level_dat_keys {
            println!("  {key}");
        }
    }
    println!("dimensions");
    for dimension in &report.dimensions {
        println!("  {:<12} {} chunks", dimension.dimension, dimension.chunks);
    }
    if !report.record_counts.is_empty() {
        println!("records");
        for (kind, count) in report.record_counts.iter().take(16) {
            println!("  {:<32} {count}", kind);
        }
    }
    if let Some(deep) = &report.deep {
        println!("decoded chunks      {}", deep.chunks);
        println!("sections            {}", deep.sections);
        println!("non-empty sections  {}", deep.non_empty_sections);
        println!("block entities      {}", deep.block_entities);
        println!("entities            {}", deep.entities);
        println!(
            "section Y range     {}..={}",
            deep.min_section_y, deep.max_section_y
        );
        if !deep.statuses.is_empty() {
            println!("statuses");
            for (status, count) in &deep.statuses {
                println!("  {:<24} {count}", if status.is_empty() { "(none)" } else { status });
            }
        }
    }
}

fn convert(args: ConvertArgs) -> Result<()> {
    if args.dry_run {
        let edition = detect_edition(&args.source)?;
        let target = args.to.unwrap_or(match edition {
            Edition::Java => Edition::Bedrock,
            Edition::Bedrock => Edition::Java,
        });
        let world = match edition {
            Edition::Java => {
                let world = JavaWorld::open(&args.source)?;
                format!(
                    "{} (DataVersion {}, {} players)",
                    world.level_name(),
                    world.data_version(),
                    world.player_files()?.len()
                )
            }
            Edition::Bedrock => {
                let summary = be::inspect::summarize(&args.source)?;
                format!("{} ({} players)", summary.level_name, summary.players)
            }
        };
        println!(
            "would convert {} [{}] -> {} at {}",
            world,
            edition.as_str(),
            target.as_str(),
            args.target.display()
        );
        return Ok(());
    }

    let mut plan = ConversionPlan::new(&args.source, &args.target);
    plan.target_edition = args.to;
    plan.target_version = args.target_version;
    plan.jobs = args.jobs;
    plan.compression_level = args.compression;
    plan.overwrite = args.overwrite;
    let options = ConversionOptions::default();
    let report = worldport_core::convert::run(&plan, &options)?;
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = &args.report {
        std::fs::write(path, &json).with_context(|| format!("write {}", path.display()))?;
    }
    println!("{json}");
    if !report.is_clean() {
        eprintln!(
            "conversion finished with {} warning rows and {} unmapped identifiers",
            report.warnings.len(),
            report.unmapped.total()
        );
    }
    Ok(())
}

#[allow(dead_code)]
fn require_dir(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("{} is not a directory", path.display());
    }
    Ok(())
}
