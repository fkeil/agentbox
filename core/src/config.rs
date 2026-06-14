use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BoxConfig {
    pub agent: AgentId,
    pub folder: FolderConfig,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub network: NetworkMode,
    #[serde(default)]
    pub resources: ResourceConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct AgentId(pub String);

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FolderConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub sync: SyncMode,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    #[default]
    Mount,
    Snapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    #[default]
    Ephemeral,
    Persistent,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: ProviderType,
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub auth: String,
    #[serde(default)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderType {
    Anthropic,
    Openai,
    OpenaiCompatible,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    #[default]
    Open,
    Allowlist,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ResourceConfig {
    pub cpus: Option<f64>,
    pub memory: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),
    #[error("validation failed:\n{}", .0.join("\n"))]
    Validation(Vec<String>),
    #[error("invalid memory value `{value}`: {reason}")]
    InvalidMemory { value: String, reason: String },
}

pub fn parse_config(path: &Path) -> Result<BoxConfig, ConfigError> {
    let content = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_owned(),
        source: e,
    })?;
    let cfg: BoxConfig = serde_yaml::from_str(&content)?;
    Ok(cfg)
}

pub fn validate_config(cfg: &BoxConfig) -> Result<(), ConfigError> {
    let mut errors: Vec<String> = Vec::new();

    if cfg.lifecycle != Lifecycle::Ephemeral {
        errors.push("lifecycle: only `ephemeral` is supported in Phase 1".into());
    }
    if cfg.folder.sync != SyncMode::Mount {
        errors.push("folder.sync: only `mount` is supported in Phase 1".into());
    }
    if cfg.network != NetworkMode::Open {
        errors.push("network: only `open` is supported in Phase 1".into());
    }
    if !cfg.folder.path.exists() {
        errors.push(format!(
            "folder.path `{}` does not exist",
            cfg.folder.path.display()
        ));
    } else if !cfg.folder.path.is_dir() {
        errors.push(format!(
            "folder.path `{}` is not a directory",
            cfg.folder.path.display()
        ));
    }
    if let Some(mem) = &cfg.resources.memory {
        if let Err(e) = parse_memory_bytes(mem) {
            errors.push(format!("resources.memory: {e}"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(errors))
    }
}

pub fn parse_memory_bytes(s: &str) -> Result<u64, ConfigError> {
    let s = s.trim();
    let (num_str, unit) = if s.ends_with(['g', 'G']) {
        (&s[..s.len() - 1], 1u64 << 30)
    } else if s.ends_with(['m', 'M']) {
        (&s[..s.len() - 1], 1u64 << 20)
    } else if s.ends_with(['k', 'K']) {
        (&s[..s.len() - 1], 1u64 << 10)
    } else if s.ends_with(['b', 'B']) {
        (&s[..s.len() - 1], 1u64)
    } else {
        (s, 1u64)
    };
    let n: u64 = num_str.parse().map_err(|_| ConfigError::InvalidMemory {
        value: s.to_owned(),
        reason: "expected a number followed by g/m/k/b (e.g. `4g`, `512m`)".into(),
    })?;
    Ok(n * unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_memory() {
        assert_eq!(parse_memory_bytes("4g").unwrap(), 4 * (1 << 30));
        assert_eq!(parse_memory_bytes("512m").unwrap(), 512 * (1 << 20));
        assert_eq!(parse_memory_bytes("1G").unwrap(), 1 << 30);
        assert!(parse_memory_bytes("abc").is_err());
    }

    #[test]
    fn parse_minimal_yaml() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-5
  auth: "none"
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.agent.0, "claude-code");
        assert_eq!(cfg.provider.provider_type, ProviderType::Anthropic);
        assert_eq!(cfg.lifecycle, Lifecycle::Ephemeral);
    }
}
