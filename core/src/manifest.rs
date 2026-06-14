use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::ProviderType;

#[derive(Debug, Deserialize)]
pub struct AgentManifest {
    pub id: String,
    pub display_name: String,
    pub base_image: String,
    pub install: InstallConfig,
    pub supported_providers: Vec<ProviderType>,
    #[serde(default)]
    pub auth: HashMap<ProviderType, AuthConfig>,
    pub config: Option<ConfigDef>,
    pub launch: LaunchConfig,
    pub workdir: String,
}

#[derive(Debug, Deserialize)]
pub struct InstallConfig {
    pub method: InstallMethod,
    pub packages: Vec<String>,
    #[serde(default)]
    pub apt_deps: Vec<String>,
}

impl InstallConfig {
    pub fn build_command(&self) -> Vec<String> {
        let mut cmd = String::new();
        if !self.apt_deps.is_empty() {
            cmd.push_str("apt-get update -qq && apt-get install -y -qq ");
            cmd.push_str(&self.apt_deps.join(" "));
            cmd.push_str(" 2>/dev/null; ");
        }
        match self.method {
            InstallMethod::Npm => {
                cmd.push_str("npm install -g ");
                cmd.push_str(&self.packages.join(" "));
            }
            InstallMethod::Pip => {
                cmd.push_str("pip install --quiet ");
                cmd.push_str(&self.packages.join(" "));
            }
        }
        vec!["sh".into(), "-c".into(), cmd]
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallMethod {
    Npm,
    Pip,
}

#[derive(Debug, Deserialize, Default)]
pub struct AuthConfig {
    pub api_key_env: Option<String>,
    pub base_url_env: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigDef {
    pub path: String,
    /// Single template used for all provider types.
    pub template: Option<String>,
    /// Per-provider-type templates. Takes precedence over `template`.
    #[serde(default)]
    pub by_provider_type: HashMap<ProviderType, String>,
}

impl ConfigDef {
    pub fn template_for(&self, provider_type: &ProviderType) -> Option<&str> {
        if !self.by_provider_type.is_empty() {
            self.by_provider_type.get(provider_type).map(|s| s.as_str())
        } else {
            self.template.as_deref()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LaunchConfig {
    pub command: Vec<String>,
    /// Universal extra args, used when args_by_provider_type has no match.
    #[serde(default)]
    pub args: Vec<String>,
    /// Per-provider-type args. Takes precedence over `args`.
    #[serde(default)]
    pub args_by_provider_type: HashMap<ProviderType, Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("cannot read manifest {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("YAML parse error in {path}: {source}")]
    Parse { path: PathBuf, source: serde_yaml::Error },
}

/// Validate a loaded manifest. Returns a list of error strings; empty = valid.
pub fn validate_manifest(m: &AgentManifest) -> Vec<String> {
    let mut errors = Vec::new();

    if m.id.is_empty() {
        errors.push("id is required".into());
    }
    if m.base_image.is_empty() {
        errors.push("base_image is required".into());
    }
    if m.install.packages.is_empty() {
        errors.push("install.packages must not be empty".into());
    }
    if m.supported_providers.is_empty() {
        errors.push("supported_providers must not be empty".into());
    }
    if m.launch.command.is_empty() {
        errors.push("launch.command must not be empty".into());
    }
    if m.workdir.is_empty() {
        errors.push("workdir is required".into());
    }

    if let Some(config) = &m.config {
        if config.template.is_none() && config.by_provider_type.is_empty() {
            errors.push("config must have either 'template' or 'by_provider_type'".into());
        }
        if !config.by_provider_type.is_empty() {
            for pt in &m.supported_providers {
                if !config.by_provider_type.contains_key(pt) {
                    let name = match pt {
                        ProviderType::Anthropic => "anthropic",
                        ProviderType::Openai => "openai",
                        ProviderType::OpenaiCompatible => "openai-compatible",
                    };
                    errors.push(format!(
                        "config.by_provider_type is missing template for '{name}'"
                    ));
                }
            }
        }
    }

    errors
}

pub fn load_manifest(path: &Path) -> Result<AgentManifest, ManifestError> {
    let content = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
        path: path.to_owned(),
        source: e,
    })?;
    serde_yaml::from_str(&content).map_err(|e| ManifestError::Parse {
        path: path.to_owned(),
        source: e,
    })
}

/// Find a manifest for `agent_id` in `dir` by looking for `{dir}/{agent_id}.yaml`.
pub fn find_manifest(dir: &Path, agent_id: &str) -> Option<AgentManifest> {
    let path = dir.join(format!("{agent_id}.yaml"));
    if !path.exists() {
        return None;
    }
    match load_manifest(&path) {
        Ok(m) => {
            let errs = validate_manifest(&m);
            if !errs.is_empty() {
                eprintln!("Warning: manifest {} has errors:", path.display());
                for e in &errs {
                    eprintln!("  - {e}");
                }
            }
            Some(m)
        }
        Err(e) => {
            eprintln!("Warning: failed to load manifest {}: {e}", path.display());
            None
        }
    }
}

/// Return `(id, display_name)` for every parseable `*.yaml` file in `dir`.
pub fn list_manifests(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<(String, String)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "yaml" {
                return None;
            }
            load_manifest(&p).ok().map(|m| (m.id, m.display_name))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{manifest_agent::ManifestAgentDef, AgentDef};
    use crate::config::{ProviderConfig, ProviderType};

    fn openai_compat_provider(name: &str, model: &str, base_url: &str) -> ProviderConfig {
        ProviderConfig {
            name: name.into(),
            provider_type: ProviderType::OpenaiCompatible,
            model: model.into(),
            base_url: Some(base_url.into()),
            auth: "none".into(),
            raw: serde_json::Value::Null,
        }
    }

    fn load_real(id: &str) -> ManifestAgentDef {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("manifests");
        let m = find_manifest(&dir, id).unwrap_or_else(|| panic!("manifest {id} not found"));
        ManifestAgentDef::new(m)
    }

    #[test]
    fn opencode_manifest_renders_openai_compat() {
        let agent = load_real("opencode");
        let provider = openai_compat_provider("local-ollama", "gemma4:latest", "http://host:11434/v1");
        let bytes = agent.render_config(&provider, None).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["model"], "local-ollama/gemma4:latest");
        assert_eq!(v["provider"]["local-ollama"]["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(v["provider"]["local-ollama"]["options"]["baseURL"], "http://host:11434/v1");
    }

    #[test]
    fn opencode_manifest_launch_args() {
        let agent = load_real("opencode");
        let provider = openai_compat_provider("local-ollama", "gemma4:latest", "http://host:11434/v1");
        let args = agent.launch_args(&provider);
        assert_eq!(args, vec!["-m", "local-ollama/gemma4:latest"]);
    }

    #[test]
    fn pi_manifest_no_config_file() {
        let agent = load_real("pi");
        assert!(agent.config_file_path().is_none());
    }

    #[test]
    fn pi_manifest_launch_args_anthropic() {
        let agent = load_real("pi");
        let provider = ProviderConfig {
            name: "anthropic".into(),
            provider_type: ProviderType::Anthropic,
            model: "claude-sonnet-4-6".into(),
            base_url: None,
            auth: "none".into(),
            raw: serde_json::Value::Null,
        };
        let args = agent.launch_args(&provider);
        assert_eq!(args, vec!["--model", "anthropic/claude-sonnet-4-6"]);
    }

    #[test]
    fn pi_manifest_launch_args_openai_compat() {
        let agent = load_real("pi");
        let provider = openai_compat_provider("my-server", "llama3", "http://localhost:8080/v1");
        let args = agent.launch_args(&provider);
        assert_eq!(args, vec!["--provider", "openai", "--model", "llama3"]);
    }

    #[test]
    fn install_command_includes_apt_deps() {
        let agent = load_real("claude-code");
        let cmd = agent.install_command();
        assert_eq!(cmd[0], "sh");
        assert!(cmd[2].contains("build-essential"));
        assert!(cmd[2].contains("npm install -g"));
    }

    #[test]
    fn manifest_overrides_builtin() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("manifests");
        let found = crate::agents::find_agent("claude-code", Some(&dir));
        assert!(found.is_some());
        // The returned agent should be the manifest variant (same id regardless).
        assert_eq!(found.unwrap().id(), "claude-code");
    }
}

/// List agent IDs for all `*.yaml` files in `dir`.
pub fn list_manifest_ids(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? == "yaml" {
                p.file_stem()?.to_str().map(String::from)
            } else {
                None
            }
        })
        .collect()
}
