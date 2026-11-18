//! Moves a Space written under the `cognitive-memory` 2.1.0 draft onto a
//! fresh KIP 2.0 Nexus (`cognitive-memory@2.0.0`). See README.md.
//!
//! Run it on a stopped host's database copy. The old engine exports every
//! element; the Space's Nexus collections are then dropped and rebuilt, and
//! the export is imported with its ids, keys, storage states and self Concept.
use clap::Parser;
use std::{collections::BTreeMap, path::PathBuf};

mod export;
mod rebuild;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// The object-store root holding the Space database (for Anda Bot,
    /// a copy of `~/.anda/db`). It is rewritten in place.
    #[arg(long)]
    db: PathBuf,
    /// The Space database name.
    #[arg(long, default_value = "anda_bot")]
    space: String,
    /// Where the export, report and option-type template are written.
    #[arg(long)]
    work: PathBuf,
    /// JSON map from each draft `Preference` Concept id to the option kind it
    /// becomes: `{"C-24": {"type": "AssistantNickname", "description": "..."}}`.
    #[arg(long)]
    types: Option<PathBuf>,
    /// Export and list the option Concepts that need a type, then stop
    /// without changing the database.
    #[arg(long)]
    list_options: bool,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.work)?;
    let export_path = args.work.join("export.json");
    let exported = if export_path.exists() {
        println!("reusing {}", export_path.display());
        serde_json::from_slice(&std::fs::read(&export_path)?)?
    } else {
        println!("exporting {} from {}", args.space, args.db.display());
        let exported = export::export(&args.db, &args.space).await?;
        std::fs::write(&export_path, serde_json::to_vec(&exported)?)?;
        exported
    };
    println!(
        "exported {} records, {} elements",
        exported.capsule["payload"]["records"]
            .as_array()
            .map_or(0, Vec::len),
        exported.facts.len()
    );

    let options = rebuild::option_concepts(&exported);
    if args.list_options {
        let template: BTreeMap<String, serde_json::Value> = options
            .iter()
            .map(|r| {
                (
                    r["id"].as_str().unwrap_or_default().to_string(),
                    serde_json::json!({
                        "type": "",
                        "description": "",
                        "name": r["name"],
                        "summary": r["attributes"]["description"],
                    }),
                )
            })
            .collect();
        let path = args.work.join("types.template.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&template)?)?;
        println!(
            "{} option Concepts need a type; template written to {}",
            options.len(),
            path.display()
        );
        return Ok(());
    }

    let types: BTreeMap<String, rebuild::OptionType> = match &args.types {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)?,
        None if options.is_empty() => BTreeMap::new(),
        None => return Err("--types is required: the Space has draft Preference Concepts".into()),
    };
    let mut before = census_of(&exported, &types);
    let report = rebuild::rebuild(&args.db, &args.space, &exported, &types).await?;
    // The lineage the draft kept in `derived_from` is now its Activity.
    if !report.lineage_activities.is_empty() {
        *before.entry("X active extraction".into()).or_default() += report.lineage_activities.len();
    }
    std::fs::write(
        args.work.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let after = rebuild::census(&args.db, &args.space).await?;
    std::fs::write(
        args.work.join("census.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "expected": before, "rebuilt": after.counts,
            "environment": after.environment, "installed_packages": after.packages,
        }))?,
    )?;
    println!(
        "rebuilt: {} packages, {} types defined, {} keys and {} archived states restored, {} id changes, {} warnings",
        report.packages.len(),
        report.defined_types.len(),
        report.keys_restored,
        report.archived_restored,
        report.id_changes.len(),
        report.warnings.len()
    );
    if before != after.counts {
        return Err(format!(
            "element counts differ; see {}",
            args.work.join("census.json").display()
        )
        .into());
    }
    println!("census: every kind, state and type count matches the export");
    Ok(())
}

/// The census the rebuilt Space should have: the export's, with the draft
/// Profile's references moved to 2.0.0 and each option on its chosen type.
fn census_of(
    exported: &export::Exported,
    types: &BTreeMap<String, rebuild::OptionType>,
) -> BTreeMap<String, usize> {
    let mut census = BTreeMap::new();
    for fact in &exported.facts {
        let schema_ref = match types.get(&fact.id) {
            Some(option) => format!("kip://local/draft@0.0.0/{}", option.name),
            None => fact.schema_ref.replace(
                "kip://profiles/cognitive-memory@2.1.0/",
                "kip://profiles/cognitive-memory@2.0.0/",
            ),
        };
        *census
            .entry(format!("{} {} {}", fact.kind, fact.state, schema_ref))
            .or_default() += 1;
    }
    census
}
