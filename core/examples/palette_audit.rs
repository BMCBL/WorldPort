//! Prints the mapping coverage of every bundled version, in both directions.
//!
//! The table makes a version-to-version change legible in a diff; the defect list
//! is the actionable part: an undeclared loss means information disappeared
//! without the conversion saying so.

use std::collections::BTreeMap;

use worldport_core::mapping::audit::{Coverage, Outcome, audit};
use worldport_core::mapping::schema::bedrock_catalog;
use worldport_core::mapping::{BlockMapper, JavaCatalog};

struct Row {
    version: String,
    direction: &'static str,
    coverage: Coverage,
}

fn main() {
    // `--json` prints the shape the coverage gate reads, so the baseline is
    // regenerated from the same code that checks it.
    let json = std::env::args().nth(1).as_deref() == Some("--json");
    let bedrock = bedrock_catalog();
    let mut rows: Vec<Row> = Vec::new();
    for (version, _) in JavaCatalog::bundled_versions() {
        let java = JavaCatalog::bundled(version).expect("bundled catalog").to_catalog();
        let mapper = BlockMapper::new(java.clone(), bedrock.clone());
        rows.push(Row {
            version: version.to_string(),
            direction: "JE->BE",
            coverage: audit(&mapper, &java, &bedrock, true),
        });
        rows.push(Row {
            version: version.to_string(),
            direction: "BE->JE",
            coverage: audit(&mapper, &bedrock, &java, false),
        });
    }

    for version in worldport_core::mapping::schema::bedrock_catalog_versions() {
        let catalog = worldport_core::mapping::schema::bedrock_catalog_for(version)
            .expect("bundled bedrock catalog");
        let java = JavaCatalog::bundled("26.2").expect("newest bundled Java catalog").to_catalog();
        let mapper = BlockMapper::new(java.clone(), catalog.clone());
        rows.push(Row {
            version: format!("be {version}"),
            direction: "BE->JE",
            coverage: audit(&mapper, &catalog, &java, false),
        });
    }

    if json {
        let mut out: BTreeMap<String, BTreeMap<&str, u64>> = BTreeMap::new();
        for row in &rows {
            let coverage = &row.coverage;
            out.insert(
                format!("{} {}", row.version, row.direction),
                BTreeMap::from([
                    ("states", coverage.states),
                    ("exact", coverage.exact()),
                    ("declared_loss", coverage.declared_losses()),
                    ("silent_loss", coverage.count(&Outcome::SilentLoss)),
                    ("unstable", coverage.count(&Outcome::Unstable)),
                    ("unmapped", coverage.count(&Outcome::Unmapped)),
                ]),
            );
        }
        println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
        return;
    }

    println!(
        "{:<15} {:<7} {:>7} {:>6} {:>9} {:>9} {:>9} {:>8} {:>10}",
        "version", "dir", "states", "exact", "declared", "silent", "unstable", "unmapped", "invalid"
    );
    for row in &rows {
        let coverage = &row.coverage;
        println!(
            "{:<15} {:<7} {:>7} {:>6} {:>9} {:>9} {:>9} {:>8} {:>10}",
            row.version,
            row.direction,
            coverage.states,
            coverage.exact(),
            coverage.declared_losses(),
            coverage.count(&Outcome::SilentLoss),
            coverage.count(&Outcome::Unstable),
            coverage.count(&Outcome::Unmapped),
            coverage.count(&Outcome::InvalidTarget),
        );
    }

    for direction in ["JE->BE", "BE->JE"] {
        let mut totals = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
        for row in rows.iter().filter(|row| row.direction == direction) {
            let coverage = &row.coverage;
            totals.0 += coverage.states;
            totals.1 += coverage.exact();
            totals.2 += coverage.declared_losses();
            totals.3 += coverage.count(&Outcome::SilentLoss);
            totals.4 += coverage.count(&Outcome::Unstable);
            totals.5 += coverage.count(&Outcome::Unmapped);
        }
        println!(
            "\n{direction}: {} states | exact {} ({:.1}%) | declared loss {} | silent loss {} | \
             unstable {} | unmapped {} ({:.1}% mapped)",
            totals.0,
            totals.1,
            totals.1 as f64 * 100.0 / totals.0 as f64,
            totals.2,
            totals.3,
            totals.4,
            totals.5,
            (totals.0 - totals.5) as f64 * 100.0 / totals.0 as f64,
        );
    }

    println!("\nUndeclared losses (fix the table or declare the loss):");
    for direction in ["JE->BE", "BE->JE"] {
        let mut shown = 0_usize;
        println!("  {direction}");
        for row in rows.iter().filter(|row| row.direction == direction) {
            for defect in &row.coverage.defects {
                if shown == 12 {
                    break;
                }
                println!("    {:<7} {defect}", row.version);
                shown += 1;
            }
        }
        if shown == 0 {
            println!("    none");
        }
    }

    println!("\nTop unmapped blocks:");
    for direction in ["JE->BE", "BE->JE"] {
        let mut merged: BTreeMap<String, u64> = BTreeMap::new();
        for row in rows.iter().filter(|row| row.direction == direction) {
            for (name, count) in &row.coverage.unmapped_blocks {
                *merged.entry(name.clone()).or_default() += count;
            }
        }
        let mut list: Vec<_> = merged.into_iter().collect();
        list.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
        println!("  {direction}: {} distinct blocks", list.len());
        for (name, count) in list.iter().take(10) {
            println!("    {count:>6}  {name}");
        }
    }
}
