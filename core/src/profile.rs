//! Named provider + agent presets stored in `~/.config/agentbox/profiles/`.
//!
//! A profile captures the fields a user wants to reuse across many boxes:
//! agent, provider, network, resources, extra_env. The `folder` and `name`
//! fields are always supplied at invocation time (via CLI flags or the wizard).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{BackendChoice, Lifecycle, NetworkMode, ProviderConfig, ResourceConfig};

/// A saved preset for quick box creation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Profile name — matches the filename stem.
    pub name: String,
    /// Agent ID (manifest id or built-in).
    pub agent: String,
    /// Provider configuration (same as box.yaml provider block).
    pub provider: ProviderConfig,
    #[serde(default)]
    pub network: NetworkMode,
    #[serde(default)]
    pub resources: ResourceConfig,
    #[serde(default)]
    pub extra_env: HashMap<String, String>,
    #[serde(default)]
    pub backend: BackendChoice,
    /// Optional default lifecycle applied when not overridden at invocation.
    #[serde(default)]
    pub lifecycle: Lifecycle,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("cannot read profiles directory `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("YAML error in profile `{name}`: {source}")]
    Parse {
        name: String,
        source: serde_yaml::Error,
    },
    #[error("profile `{0}` not found")]
    NotFound(String),
    #[error("profile `{0}` already exists; use --force to overwrite")]
    AlreadyExists(String),
}

/// Return the user profile directory, creating it if absent.
pub fn profiles_dir() -> PathBuf {
    let base = dirs_home()
        .join(".config")
        .join("agentbox")
        .join("profiles");
    let _ = std::fs::create_dir_all(&base);
    base
}

/// Return the user manifest directory (for `agentbox manifest add`).
pub fn user_manifests_dir() -> PathBuf {
    let base = dirs_home()
        .join(".config")
        .join("agentbox")
        .join("manifests");
    let _ = std::fs::create_dir_all(&base);
    base
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// List all profiles in the profiles directory.
pub fn list_profiles() -> Result<Vec<Profile>, ProfileError> {
    let dir = profiles_dir();
    let entries = std::fs::read_dir(&dir).map_err(|e| ProfileError::Io {
        path: dir.clone(),
        source: e,
    })?;

    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            if let Some(p) = load_profile_file(&path) {
                profiles.push(p);
            }
        }
    }
    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Load a profile by name.
pub fn load_profile(name: &str) -> Result<Profile, ProfileError> {
    let path = profiles_dir().join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ProfileError::NotFound(name.to_string()));
    }
    load_profile_file(&path).ok_or_else(|| ProfileError::NotFound(name.to_string()))
}

fn load_profile_file(path: &Path) -> Option<Profile> {
    let content = std::fs::read_to_string(path).ok()?;
    let name = path.file_stem()?.to_str()?.to_string();
    let mut p: Profile = serde_yaml::from_str(&content).ok()?;
    p.name = name;
    Some(p)
}

/// Save a profile to the profiles directory.
pub fn save_profile(profile: &Profile, force: bool) -> Result<PathBuf, ProfileError> {
    let path = profiles_dir().join(format!("{}.yaml", profile.name));
    if path.exists() && !force {
        return Err(ProfileError::AlreadyExists(profile.name.clone()));
    }
    let content = serde_yaml::to_string(profile).map_err(|e| ProfileError::Parse {
        name: profile.name.clone(),
        source: e,
    })?;
    std::fs::write(&path, content).map_err(|e| ProfileError::Io {
        path: path.clone(),
        source: e,
    })?;
    Ok(path)
}

/// Serialize a profile to a YAML string (for sharing / export).
pub fn export_profile_yaml(name: &str) -> Result<String, ProfileError> {
    let profile = load_profile(name)?;
    serde_yaml::to_string(&profile).map_err(|e| ProfileError::Parse {
        name: name.to_string(),
        source: e,
    })
}

/// Deserialize a profile from YAML and save it (for import).
/// If `name_override` is given, the profile is renamed before saving.
pub fn import_profile_yaml(
    yaml: &str,
    name_override: Option<&str>,
    force: bool,
) -> Result<PathBuf, ProfileError> {
    let mut profile: Profile = serde_yaml::from_str(yaml).map_err(|e| ProfileError::Parse {
        name: "<import>".to_string(),
        source: e,
    })?;
    if let Some(n) = name_override {
        profile.name = n.to_string();
    }
    save_profile(&profile, force)
}

/// Remove a profile by name.
pub fn remove_profile(name: &str) -> Result<(), ProfileError> {
    let path = profiles_dir().join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ProfileError::NotFound(name.to_string()));
    }
    std::fs::remove_file(&path).map_err(|e| ProfileError::Io { path, source: e })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfig, ProviderType};

    fn test_profile(name: &str) -> Profile {
        Profile {
            name: name.to_string(),
            agent: "claude-code".to_string(),
            provider: ProviderConfig {
                name: "anthropic".to_string(),
                provider_type: ProviderType::Anthropic,
                model: "claude-sonnet-4-6".to_string(),
                base_url: None,
                auth: "none".to_string(),
                raw: serde_json::Value::Null,
            },
            network: NetworkMode::Open,
            resources: ResourceConfig::default(),
            extra_env: HashMap::new(),
            backend: BackendChoice::Auto,
            lifecycle: Lifecycle::Ephemeral,
        }
    }

    #[test]
    fn profile_round_trips_through_yaml() {
        let p = test_profile("my-test");
        let yaml = serde_yaml::to_string(&p).unwrap();
        let p2: Profile = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p2.agent, "claude-code");
        assert_eq!(p2.provider.model, "claude-sonnet-4-6");
    }

    #[test]
    fn into_box_config_merges_folder() {
        let p = test_profile("test");
        let cfg = p.into_box_config(
            PathBuf::from("/tmp/my-project"),
            Some("my-box".to_string()),
            None,
        );
        assert_eq!(cfg.folder.path, PathBuf::from("/tmp/my-project"));
        assert_eq!(cfg.name.as_deref(), Some("my-box"));
        assert_eq!(cfg.agent.0, "claude-code");
    }

    #[test]
    fn into_box_config_lifecycle_override() {
        let p = test_profile("test");
        let cfg = p.into_box_config(PathBuf::from("/tmp"), None, Some(Lifecycle::Persistent));
        assert_eq!(cfg.lifecycle, Lifecycle::Persistent);
    }

    #[test]
    fn export_and_import_yaml_round_trip() {
        let p = test_profile("export-test");
        let yaml = serde_yaml::to_string(&p).unwrap();

        // import with same name
        let p2 = import_profile_yaml(&yaml, None, true).unwrap();
        let loaded = load_profile_file(&p2).unwrap();
        assert_eq!(loaded.agent, "claude-code");
        assert_eq!(loaded.provider.model, "claude-sonnet-4-6");

        // import with a different name
        let p3 = import_profile_yaml(&yaml, Some("renamed-profile"), true).unwrap();
        let loaded3 = load_profile_file(&p3).unwrap();
        assert_eq!(loaded3.name, "renamed-profile");

        // cleanup
        std::fs::remove_file(p2).ok();
        std::fs::remove_file(p3).ok();
    }

    #[test]
    fn import_invalid_yaml_errors() {
        let result = import_profile_yaml("not: valid: yaml: [[[", None, true);
        assert!(result.is_err());
    }
}

impl Profile {
    /// Merge this profile with an override folder path and optional name,
    /// producing a full `BoxConfig` ready to pass to the engine.
    pub fn into_box_config(
        self,
        folder_path: PathBuf,
        box_name: Option<String>,
        lifecycle_override: Option<Lifecycle>,
    ) -> crate::config::BoxConfig {
        let lifecycle = lifecycle_override.unwrap_or(self.lifecycle);
        crate::config::BoxConfig {
            agent: crate::config::AgentId(self.agent),
            name: box_name,
            project_name: None,
            folder: crate::config::FolderConfig {
                path: folder_path,
                sync: crate::config::SyncMode::Mount,
            },
            lifecycle,
            provider: self.provider,
            network: self.network,
            resources: self.resources,
            extra_env: self.extra_env,
            backend: self.backend,
            hooks: Default::default(),
            extra_mounts: vec![],
            notifications: false,
            remote: None,
        }
    }
}
