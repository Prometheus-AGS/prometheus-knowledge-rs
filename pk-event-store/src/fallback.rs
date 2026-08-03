//! Offline fallback: append events to a JSONL file when SurrealDB is unavailable.

use crate::EventRecord;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct JsonlFallback {
    path: PathBuf,
}

impl JsonlFallback {
    pub fn new(project_root: &Path) -> Self {
        let path = project_root.join(".prometheus").join("events.jsonl");
        Self { path }
    }

    pub fn append(&self, record: &EventRecord) -> Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(record)? + "\n";
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Read the last `limit` records from the fallback file.
    pub fn tail(&self, limit: usize) -> Result<Vec<EventRecord>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut records: Vec<EventRecord> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        records.reverse(); // most recent first
        records.truncate(limit);
        Ok(records)
    }

    /// Filter by project root and optional kind.
    pub fn list(
        &self,
        project_root: &str,
        kind: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventRecord>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut records: Vec<EventRecord> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<EventRecord>(l).ok())
            .filter(|r| r.project_root == project_root)
            .filter(|r| kind.is_none_or(|k| r.kind == k))
            .collect();
        records.sort_by_key(|record| std::cmp::Reverse(record.timestamp));
        records.truncate(limit);
        Ok(records)
    }

    /// Return all events that affect a given entry ID.
    pub fn for_entry(&self, entry_id: &str, project_root: &str) -> Result<Vec<EventRecord>> {
        if !self.path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&self.path)?;
        let mut records: Vec<EventRecord> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<EventRecord>(l).ok())
            .filter(|r| r.project_root == project_root && r.affects.iter().any(|a| a == entry_id))
            .collect();
        records.sort_by_key(|record| record.timestamp);
        Ok(records)
    }
}
