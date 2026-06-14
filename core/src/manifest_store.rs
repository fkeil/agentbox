//! User-managed manifest store at `~/.config/agentbox/manifests/`.
//!
//! Manifests from this directory take precedence over bundled manifests
//! (they shadow by id), but bundled manifests ship with the binary so they
//! are always available without any setup.

use std::path::{Path, PathBuf};

use crate::manifest::AgentManifest;
use crate::profile::user_manifests_dir;

#[derive(Debug, thiserror::Error)]
pub enum ManifestStoreError {
    #[error("I/O error for `{path}`: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("YAML parse error in `{path}`: {source}")]
    Parse { path: PathBuf, source: serde_yaml::Error },
    #[error("network error downloading manifest: {0}")]
    Network(#[from] reqwest::Error),
    #[error("manifest `{0}` not found in user store")]
    NotFound(String),
    #[error("manifest `{0}` already exists; use --force to overwrite")]
    AlreadyExists(String),
}

/// Download or copy a manifest from `source` (URL or file path) into the
/// user manifest directory. Validates that it parses as a valid manifest.
pub async fn add_manifest(
    source: &str,
    force: bool,
) -> Result<(String, PathBuf), ManifestStoreError> {
    let content = if source.starts_with("http://") || source.starts_with("https://") {
        tracing::debug!(url = source, "downloading manifest");
        reqwest::get(source).await?.text().await?
    } else {
        let path = PathBuf::from(source);
        std::fs::read_to_string(&path).map_err(|e| ManifestStoreError::Io {
            path: path.clone(),
            source: e,
        })?
    };

    // Validate before saving.
    let manifest: AgentManifest =
        serde_yaml::from_str(&content).map_err(|e| ManifestStoreError::Parse {
            path: PathBuf::from(source),
            source: e,
        })?;

    let dest_dir = user_manifests_dir();
    let dest = dest_dir.join(format!("{}.yaml", manifest.id));

    if dest.exists() && !force {
        return Err(ManifestStoreError::AlreadyExists(manifest.id.clone()));
    }

    std::fs::write(&dest, content.as_bytes()).map_err(|e| ManifestStoreError::Io {
        path: dest.clone(),
        source: e,
    })?;

    tracing::info!(id = manifest.id, dest = %dest.display(), "manifest installed");
    Ok((manifest.id, dest))
}

/// Remove a manifest from the user store by agent id.
pub fn remove_manifest(agent_id: &str) -> Result<(), ManifestStoreError> {
    let path = user_manifests_dir().join(format!("{agent_id}.yaml"));
    if !path.exists() {
        return Err(ManifestStoreError::NotFound(agent_id.to_string()));
    }
    std::fs::remove_file(&path).map_err(|e| ManifestStoreError::Io {
        path,
        source: e,
    })
}

/// List all manifests in the user store.
pub fn list_user_manifests() -> Vec<UserManifestEntry> {
    let dir = user_manifests_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut result: Vec<UserManifestEntry> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("yaml") {
                return None;
            }
            let content = std::fs::read_to_string(&path).ok()?;
            let manifest: AgentManifest = serde_yaml::from_str(&content).ok()?;
            Some(UserManifestEntry {
                id: manifest.id.clone(),
                display_name: manifest.display_name.clone(),
                path,
            })
        })
        .collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    result
}

/// A manifest entry in the user store.
#[derive(Debug, Clone)]
pub struct UserManifestEntry {
    pub id: String,
    pub display_name: String,
    pub path: PathBuf,
}

/// Returns the user manifest directory path so the engine can search it.
pub fn user_manifests_search_path() -> Option<PathBuf> {
    let dir = user_manifests_dir();
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// Find a manifest by id, checking user store first then `bundled_dir`.
/// This is the single lookup point that unifies user + bundled manifests.
pub fn find_manifest_with_user_store(
    bundled_dir: Option<&Path>,
    id: &str,
) -> Option<AgentManifest> {
    // User store takes priority.
    let user_path = user_manifests_dir().join(format!("{id}.yaml"));
    if user_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&user_path) {
            if let Ok(m) = serde_yaml::from_str::<AgentManifest>(&content) {
                return Some(m);
            }
        }
    }
    // Fall back to bundled manifests dir.
    bundled_dir.and_then(|d| crate::manifest::find_manifest(d, id))
}
