//! Writes the design JSON Schema to `schemas/design.schema.json`.
//!
//! Run after any change to `design-model` types and commit the result:
//! `cargo run -p server --bin generate_schema`. CI fails when the
//! committed schema is stale.

use std::fs;
use std::path::Path;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let schema = schemars::schema_for!(design_model::Design);
    let json = serde_json::to_string_pretty(&schema).context("serialize design schema")?;

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schema_directory = workspace_root.join("schemas");
    fs::create_dir_all(&schema_directory).context("create schemas/ directory")?;

    let schema_path = schema_directory.join("design.schema.json");
    fs::write(&schema_path, json + "\n")
        .with_context(|| format!("write {}", schema_path.display()))?;
    println!("wrote {}", schema_path.display());
    Ok(())
}
