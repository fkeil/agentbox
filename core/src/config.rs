use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BoxConfig {
    pub agent: AgentId,
    /// Box name — required when `lifecycle: persistent`, ignored for ephemeral.
    #[serde(default)]
    pub name: Option<String>,
    /// Optional human-readable project name shown in window titles and box lists.
    #[serde(default)]
    pub project_name: Option<String>,
    pub folder: FolderConfig,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub network: NetworkMode,
    /// Outbound network (egress) control. Replaces/extends the legacy `network: allowlist`.
    #[serde(default)]
    pub egress: EgressConfig,
    #[serde(default)]
    pub resources: ResourceConfig,
    /// Arbitrary extra env vars injected into the container. Values support
    /// the same `${env:…}` / `${file:…}` / `${keychain:…}` syntax as `auth`,
    /// and also bare literal strings.
    #[serde(default)]
    pub extra_env: HashMap<String, String>,
    /// Container backend to use. `auto` (default) tries Docker then Podman.
    #[serde(default)]
    pub backend: BackendChoice,
    /// Shell commands to run on the host before/after the container session.
    #[serde(default)]
    pub hooks: HooksConfig,
    /// Additional host directories to bind-mount into the container (read-only by default).
    #[serde(default)]
    pub extra_mounts: Vec<ExtraMount>,
    /// Send an OS desktop notification when the session ends.
    #[serde(default)]
    pub notifications: bool,
    /// Remote Docker host (e.g. `ssh://user@host`). Overrides `DOCKER_HOST`.
    #[serde(default)]
    pub remote: Option<String>,
}

/// Host shell commands executed before and after a container session.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct HooksConfig {
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
}

/// An extra bind-mount added alongside the primary workspace folder.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExtraMount {
    /// Absolute or relative path on the host.
    pub path: PathBuf,
    /// Absolute path inside the container.
    pub container_path: String,
    /// Mount as read-only (default: true).
    #[serde(default = "default_true")]
    pub readonly: bool,
}

fn default_true() -> bool {
    true
}

/// Which container backend (or isolation layer) to use.
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackendChoice {
    /// Detect Docker first, then Podman (default).
    #[default]
    Auto,
    /// Force Docker socket (`/var/run/docker.sock` or `DOCKER_HOST`).
    Docker,
    /// Force Podman socket (`$XDG_RUNTIME_DIR/podman/podman.sock`).
    Podman,
    /// microVM isolation (firecracker/QEMU) — future; currently returns an error.
    Microvm,
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
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

/// Outbound network (egress) control for a box.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EgressConfig {
    /// Default action when no rule matches. Defaults to `allow`.
    #[serde(default)]
    pub default: EgressPolicy,
    /// Destinations always permitted (evaluated after deny; deny wins).
    /// Each entry: CIDR, IP, exact hostname, `*.wildcard`, or preset name.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Destinations always blocked.
    #[serde(default)]
    pub deny: Vec<String>,
}

impl EgressConfig {
    /// True when no egress filtering is configured (open network).
    pub fn is_unrestricted(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty() && self.default == EgressPolicy::Allow
    }
}

/// Default action when no egress rule matches.
#[derive(Debug, Clone, Deserialize, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EgressPolicy {
    /// Permit all traffic not explicitly denied (default).
    #[default]
    Allow,
    /// Block all traffic not explicitly allowed.
    Deny,
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

    if cfg.lifecycle == Lifecycle::Persistent && cfg.name.is_none() {
        errors.push("name is required when lifecycle is `persistent`".into());
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
    for (i, mount) in cfg.extra_mounts.iter().enumerate() {
        if !mount.path.exists() {
            errors.push(format!(
                "extra_mounts[{i}].path `{}` does not exist",
                mount.path.display()
            ));
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

    #[test]
    fn allowlist_mode_passes_validation() {
        let mut cfg: BoxConfig = serde_yaml::from_str(
            r#"
agent: claude-code
folder:
  path: .
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-5
  auth: "none"
network: allowlist
"#,
        )
        .unwrap();
        cfg.folder.path = std::env::temp_dir();
        assert_eq!(cfg.network, NetworkMode::Allowlist);
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn daemon_requires_persistent_lifecycle() {
        // A box.yaml with lifecycle: ephemeral + a daemon agent should be caught at
        // engine level, but validate_config itself is lifecycle-agnostic.  This test
        // verifies that a persistent box without a name fails validation (the name
        // requirement is the only lifecycle check in validate_config).
        let yaml = r#"
agent: hermes
folder:
  path: /tmp
provider:
  name: openai
  type: openai
  model: gpt-4o
  auth: "none"
lifecycle: persistent
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        let err = validate_config(&cfg).unwrap_err();
        let ConfigError::Validation(msgs) = err else {
            panic!("expected Validation error");
        };
        assert!(msgs.iter().any(|m| m.contains("name is required")));
    }

    #[test]
    fn backend_choice_defaults_to_auto() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.backend, BackendChoice::Auto);
    }

    #[test]
    fn backend_choice_podman_parses() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
backend: podman
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.backend, BackendChoice::Podman);
    }

    #[test]
    fn backend_choice_microvm_parses() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
backend: microvm
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.backend, BackendChoice::Microvm);
    }

    #[test]
    fn hooks_default_empty() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.hooks.before.is_empty());
        assert!(cfg.hooks.after.is_empty());
    }

    #[test]
    fn hooks_parse() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
hooks:
  before:
    - echo pre-hook
  after:
    - echo post-hook
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.hooks.before, vec!["echo pre-hook"]);
        assert_eq!(cfg.hooks.after, vec!["echo post-hook"]);
    }

    #[test]
    fn extra_mounts_parse() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
extra_mounts:
  - path: /tmp
    container_path: /docs
  - path: /tmp
    container_path: /data
    readonly: false
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.extra_mounts.len(), 2);
        assert!(cfg.extra_mounts[0].readonly); // default is true
        assert!(!cfg.extra_mounts[1].readonly);
        assert_eq!(cfg.extra_mounts[0].container_path, "/docs");
    }

    #[test]
    fn notifications_defaults_false() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!cfg.notifications);
    }

    #[test]
    fn remote_field_parses() {
        let yaml = r#"
agent: claude-code
folder:
  path: /tmp
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: "none"
remote: ssh://user@myserver
"#;
        let cfg: BoxConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.remote.as_deref(), Some("ssh://user@myserver"));
    }
}
