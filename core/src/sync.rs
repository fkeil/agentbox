use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use similar::TextDiff;

use crate::container::{ContainerBackend, ContainerError, ContainerId, DockerBackend};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub kind: DiffKind,
    /// Unified-diff patch text (for display).
    pub patch: String,
    /// UTF-8 text of the new file (for writeback). Empty for Deleted files or
    /// binary files that could not be decoded as UTF-8.
    pub new_content: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DiffKind {
    Added,
    Modified,
    Deleted,
}

// ── Core operations ───────────────────────────────────────────────────────────

/// Download `workdir` from the running container, compare with the host
/// directory, and return a diff for every file that changed.
pub async fn compute_snapshot_diff(
    docker: &DockerBackend,
    id: &ContainerId,
    workdir: &str,
    host_dir: &Path,
) -> Result<Vec<FileDiff>, ContainerError> {
    let tar_bytes = docker.download_dir(id, workdir).await?;
    let container_files = parse_tar_to_map(&tar_bytes)?;
    let host_files = read_dir_to_map(host_dir)?;

    let mut diffs = Vec::new();

    for (path, new_text) in &container_files {
        if let Some(old_text) = host_files.get(path) {
            if old_text != new_text {
                diffs.push(FileDiff {
                    path: path.clone(),
                    kind: DiffKind::Modified,
                    patch: make_patch(path, old_text, new_text),
                    new_content: new_text.clone(),
                });
            }
        } else {
            diffs.push(FileDiff {
                path: path.clone(),
                kind: DiffKind::Added,
                patch: make_patch(path, "", new_text),
                new_content: new_text.clone(),
            });
        }
    }

    for path in host_files.keys() {
        if !container_files.contains_key(path) {
            diffs.push(FileDiff {
                path: path.clone(),
                kind: DiffKind::Deleted,
                patch: format!("--- a/{path}\n+++ /dev/null\n"),
                new_content: String::new(),
            });
        }
    }

    diffs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(diffs)
}

/// Write approved changes from a snapshot diff back to the host directory.
/// Only files in `approved_paths` are written; others are untouched.
pub fn apply_approved_changes(
    host_dir: &Path,
    diffs: &[FileDiff],
    approved_paths: &[String],
) -> Result<(), std::io::Error> {
    let approved: std::collections::HashSet<&str> =
        approved_paths.iter().map(|s| s.as_str()).collect();

    for diff in diffs {
        if !approved.contains(diff.path.as_str()) {
            continue;
        }
        let host_path = host_dir.join(&diff.path);
        match diff.kind {
            DiffKind::Deleted => {
                std::fs::remove_file(&host_path).ok();
            }
            DiffKind::Added | DiffKind::Modified => {
                if let Some(parent) = host_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&host_path, diff.new_content.as_bytes())?;
            }
        }
    }
    Ok(())
}

// ── Diff persistence ──────────────────────────────────────────────────────────

/// Where the snapshot diff for `host_dir` is stored after a run.
pub fn diff_path_for(host_dir: &Path) -> std::path::PathBuf {
    let slug: String = host_dir
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
        .chars()
        .take(40)
        .collect();
    std::env::temp_dir().join(format!("agentbox-snapshot-{slug}.json"))
}

pub fn store_diff(diffs: &[FileDiff], host_dir: &Path) -> Result<(), std::io::Error> {
    let json = serde_json::to_string(diffs).expect("serialization is infallible");
    std::fs::write(diff_path_for(host_dir), json)
}

pub fn load_diff(host_dir: &Path) -> Option<Vec<FileDiff>> {
    let json = std::fs::read_to_string(diff_path_for(host_dir)).ok()?;
    serde_json::from_str(&json).ok()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_patch(path: &str, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// Parse a tar archive (as returned by Docker's download API) into a path→text
/// map. Binary files that cannot be decoded as UTF-8 are silently skipped.
fn parse_tar_to_map(tar_bytes: &[u8]) -> Result<HashMap<String, String>, ContainerError> {
    let cursor = std::io::Cursor::new(tar_bytes);
    let mut archive = tar::Archive::new(cursor);
    let mut files = HashMap::new();

    for entry in archive.entries().map_err(|e| ContainerError::Tar(e.to_string()))? {
        let mut entry = entry.map_err(|e| ContainerError::Tar(e.to_string()))?;

        if entry.header().entry_type().is_dir() {
            continue;
        }

        let path_str = entry
            .path()
            .map_err(|e| ContainerError::Tar(e.to_string()))?
            .to_string_lossy()
            .to_string();

        // Strip leading directory component: "workspace/a/b" → "a/b"
        let rel = strip_tar_prefix(&path_str);
        if rel.is_empty() || rel.starts_with(".git/") {
            continue;
        }

        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| ContainerError::Tar(e.to_string()))?;

        if let Ok(text) = String::from_utf8(bytes) {
            files.insert(rel, text);
        }
        // Binary files are silently excluded from the diff.
    }

    Ok(files)
}

fn strip_tar_prefix(path: &str) -> String {
    let path = path.trim_start_matches("./");
    match path.find('/') {
        Some(pos) => path[pos + 1..].to_string(),
        None => String::new(), // top-level directory entry (the dir itself)
    }
}

fn read_dir_to_map(dir: &Path) -> Result<HashMap<String, String>, ContainerError> {
    let mut map = HashMap::new();
    read_dir_recursive(dir, dir, &mut map)?;
    Ok(map)
}

fn read_dir_recursive(
    base: &Path,
    dir: &Path,
    map: &mut HashMap<String, String>,
) -> Result<(), ContainerError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                continue;
            }
            read_dir_recursive(base, &path, map)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .to_string();
            // Only index text files
            if let Ok(text) = std::fs::read_to_string(&path) {
                map.insert(rel, text);
            }
        }
    }
    Ok(())
}
