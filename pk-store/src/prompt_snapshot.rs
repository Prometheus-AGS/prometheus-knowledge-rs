use pk_core::{error::PkResult, types::WikiEntry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSnapshot {
    pub schema_version: u32,
    pub scope: String,
    pub generation: String,
    pub candidate_count: usize,
    pub byte_count: usize,
    pub entries: Vec<WikiEntry>,
}

pub fn snapshot_root(knowledge_root: &Path, scope: &str) -> PathBuf {
    knowledge_root.join(".prompt-snapshots").join(scope)
}

pub fn commit_prompt_snapshot(
    knowledge_root: &Path,
    scope: &str,
    mut entries: Vec<WikiEntry>,
) -> PkResult<PromptSnapshot> {
    validate_scope(scope)?;
    entries.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let entries_bytes = serde_json::to_vec(&entries)?;
    let generation = snapshot_generation(scope, &entries_bytes);
    let snapshot = PromptSnapshot {
        schema_version: 1,
        scope: scope.to_owned(),
        generation: generation.clone(),
        candidate_count: entries.len(),
        byte_count: entries_bytes.len(),
        entries,
    };
    let encoded = serde_json::to_vec_pretty(&snapshot)?;
    let root = snapshot_root(knowledge_root, scope);
    let generations = root.join("generations");
    fs::create_dir_all(&generations)?;
    harden_directory(&knowledge_root.join(".prompt-snapshots"))?;
    harden_directory(&root)?;
    harden_directory(&generations)?;
    let generation_path = generations.join(format!("{generation}.json"));
    if !generation_path.exists() {
        write_new_synced(&generation_path, &encoded)?;
        harden_generation(&generation_path)?;
        sync_directory(&generations)?;
    }
    atomic_replace(&root.join("current"), format!("{generation}\n").as_bytes())?;
    Ok(snapshot)
}

pub fn read_prompt_snapshot(knowledge_root: &Path, scope: &str) -> PkResult<PromptSnapshot> {
    validate_scope(scope)?;
    let root = snapshot_root(knowledge_root, scope);
    let generation = fs::read_to_string(root.join("current"))?.trim().to_owned();
    if generation.len() != 64 || !generation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(pk_core::error::PkError::Other(
            "prompt snapshot pointer is not a SHA-256 generation".to_owned(),
        ));
    }
    let snapshot: PromptSnapshot = serde_json::from_slice(&fs::read(
        root.join("generations").join(format!("{generation}.json")),
    )?)?;
    let entries_bytes = serde_json::to_vec(&snapshot.entries)?;
    let expected = snapshot_generation(scope, &entries_bytes);
    if snapshot.schema_version != 1
        || snapshot.scope != scope
        || snapshot.generation != generation
        || expected != generation
        || snapshot.candidate_count != snapshot.entries.len()
        || snapshot.byte_count != entries_bytes.len()
    {
        return Err(pk_core::error::PkError::Other(
            "prompt snapshot failed identity or count validation".to_owned(),
        ));
    }
    Ok(snapshot)
}

fn snapshot_generation(scope: &str, entries_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"prometheus-prompt-snapshot-v1\0");
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(entries_bytes);
    format!("{:x}", hasher.finalize())
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> PkResult<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    harden_private_file(path)?;
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn validate_scope(scope: &str) -> PkResult<()> {
    if matches!(scope, "project" | "shared" | "global") {
        Ok(())
    } else {
        Err(pk_core::error::PkError::Other(
            "prompt snapshot scope must be project, shared, or global".to_owned(),
        ))
    }
}

#[cfg(unix)]
fn harden_directory(path: &Path) -> PkResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_directory(_path: &Path) -> PkResult<()> {
    Ok(())
}

#[cfg(unix)]
fn harden_private_file(path: &Path) -> PkResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_private_file(_path: &Path) -> PkResult<()> {
    Ok(())
}

#[cfg(unix)]
fn harden_generation(path: &Path) -> PkResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_generation(_path: &Path) -> PkResult<()> {
    Ok(())
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> PkResult<()> {
    let parent = path.parent().ok_or_else(|| {
        pk_core::error::PkError::Other("snapshot pointer has no parent".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".current.{}.{}.tmp",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_synced(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    sync_directory(parent)?;
    Ok(())
}

fn sync_directory(path: &Path) -> PkResult<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn failed_generation_does_not_replace_last_committed_pointer() {
        let temp = TempDir::new().unwrap();
        let first = vec![WikiEntry::new("First", "committed")];
        let committed = commit_prompt_snapshot(temp.path(), "project", first).unwrap();
        let pointer =
            fs::read_to_string(snapshot_root(temp.path(), "project").join("current")).unwrap();

        let generations = snapshot_root(temp.path(), "project").join("generations");
        let saved_generations = snapshot_root(temp.path(), "project").join("saved-generations");
        fs::rename(&generations, &saved_generations).unwrap();
        fs::write(&generations, b"blocks directory creation").unwrap();
        let attempted = commit_prompt_snapshot(
            temp.path(),
            "project",
            vec![WikiEntry::new("Second", "cannot commit")],
        );
        fs::remove_file(&generations).unwrap();
        fs::rename(&saved_generations, &generations).unwrap();

        assert!(attempted.is_err());
        assert_eq!(
            fs::read_to_string(snapshot_root(temp.path(), "project").join("current")).unwrap(),
            pointer
        );
        assert_eq!(
            read_prompt_snapshot(temp.path(), "project")
                .unwrap()
                .generation,
            committed.generation
        );
    }
}
