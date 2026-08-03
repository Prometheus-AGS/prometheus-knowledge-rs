//! SP-020: Migration from single-store to dual-store.
//!
//! Reads the existing JSONL event file (single-store, from SP-019) and
//! classifies each record into KG or episodic. In dry-run mode, prints the
//! classification plan without moving anything. In apply mode, moves records
//! to the correct logical store (JSONL shards or SurrealDB databases).

use crate::{dual_store, EventRecord};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct MigrationPlan {
    pub kg_records: Vec<EventRecord>,
    pub episodic_records: Vec<EventRecord>,
    pub unclassified: Vec<EventRecord>,
}

/// Read all records from the legacy JSONL fallback and classify them.
pub fn plan(project_root: &Path) -> Result<MigrationPlan> {
    let source = project_root.join(".prometheus").join("events.jsonl");
    if !source.exists() {
        return Ok(MigrationPlan {
            kg_records: vec![],
            episodic_records: vec![],
            unclassified: vec![],
        });
    }

    let content = std::fs::read_to_string(&source)?;
    let records: Vec<EventRecord> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let mut kg_records = Vec::new();
    let mut episodic_records = Vec::new();
    let unclassified = Vec::new(); // currently all records are classifiable

    for record in records {
        match dual_store::classify(&record) {
            dual_store::StoreTarget::KnowledgeGraph => kg_records.push(record),
            dual_store::StoreTarget::Episodic => episodic_records.push(record),
        }
    }

    Ok(MigrationPlan {
        kg_records,
        episodic_records,
        unclassified,
    })
}

/// Apply the migration plan: write shard files.
pub fn apply(plan: &MigrationPlan, project_root: &Path) -> Result<()> {
    let base = project_root.join(".prometheus");
    std::fs::create_dir_all(&base)?;

    write_shard(&plan.kg_records, &base.join("events-kg.jsonl"))?;
    write_shard(&plan.episodic_records, &base.join("events-episodic.jsonl"))?;

    if !plan.unclassified.is_empty() {
        write_shard(&plan.unclassified, &base.join("events-unsorted.jsonl"))?;
    }

    Ok(())
}

fn write_shard(records: &[EventRecord], path: &PathBuf) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for r in records {
        let line = serde_json::to_string(r)? + "\n";
        file.write_all(line.as_bytes())?;
    }
    Ok(())
}
