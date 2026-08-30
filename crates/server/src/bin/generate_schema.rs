//! Writes the JSON Schemas to `schemas/`: the design, the deck, the
//! document, and the question set.
//!
//! Run after any change to `design-model` types and commit the result:
//! `cargo run -p server --bin generate_schema`. CI fails when a
//! committed schema is stale.

use std::fs;
use std::path::Path;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schema_directory = workspace_root.join("schemas");
    fs::create_dir_all(&schema_directory).context("create schemas/ directory")?;
    let schemas = [
        (
            "design.schema.json",
            schemars::schema_for!(design_model::Design),
        ),
        (
            "deck.schema.json",
            schemars::schema_for!(design_model::Deck),
        ),
        (
            "document.schema.json",
            schemars::schema_for!(design_model::Document),
        ),
        (
            "question-set.schema.json",
            schemars::schema_for!(design_model::BriefQuestionSet),
        ),
    ];
    for (file_name, schema) in schemas {
        let json = serde_json::to_string_pretty(&schema)
            .with_context(|| format!("serialize {file_name}"))?;
        let schema_path = schema_directory.join(file_name);
        fs::write(&schema_path, json + "\n")
            .with_context(|| format!("write {}", schema_path.display()))?;
        println!("wrote {}", schema_path.display());
    }
    Ok(())
}
