use crate::config::{ProviderConfig, ProviderType};
use std::collections::HashMap;
use std::path::Path;

pub mod claude_code;
pub mod manifest_agent;
pub mod opencode;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("config rendering failed: {0}")]
    RenderFailed(String),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Everything the engine needs to know about an agent. Hardcoded structs
/// implement this for Phase 1 agents; ManifestAgentDef implements it for
/// YAML-defined agents loaded at runtime.
pub trait AgentDef: Send + Sync {
    fn id(&self) -> &str;
    /// Human-readable name shown in UI (e.g. "Claude Code", "OpenCode", "Pi").
    fn display_name(&self) -> &str {
        self.id()
    }
    fn base_image(&self) -> &str;
    /// Shell args passed to `exec` for installing the agent (runs as root).
    fn install_command(&self) -> Vec<String>;
    fn supported_providers(&self) -> &[ProviderType];
    /// The env var name the agent reads for its API key, per provider type.
    fn api_key_env_var(&self, provider_type: &ProviderType) -> Option<&str>;
    /// The env var name the agent reads for a custom base URL, if supported.
    fn base_url_env_var(&self, provider_type: &ProviderType) -> Option<&str>;
    /// In-container path for the native config file, or None if not needed.
    fn config_file_path(&self) -> Option<&str>;
    /// Render the native config file bytes for the given provider + key.
    fn render_config(
        &self,
        provider: &ProviderConfig,
        resolved_key: Option<&str>,
    ) -> Result<Vec<u8>, AgentError>;
    fn launch_command(&self) -> Vec<String>;
    /// Additional args appended to launch_command at runtime, given the provider.
    fn launch_args(&self, _provider: &ProviderConfig) -> Vec<String> {
        vec![]
    }
    /// In-container path where the user folder is bind-mounted.
    fn workdir(&self) -> &str;
    /// Extra env vars the agent always needs beyond auth injection.
    fn extra_env(&self, provider: &ProviderConfig) -> HashMap<String, String>;
    /// Command to verify the agent installed correctly. None = skip.
    fn healthcheck_command(&self) -> Option<Vec<String>> {
        None
    }
    /// Container path for the OAuth token cache (mounted as a named volume).
    /// None means this agent does not support in-container OAuth.
    fn oauth_cache_path(&self) -> Option<&str> {
        None
    }
    /// Daemon configuration, or None for session-mode agents.
    fn daemon_config(&self) -> Option<&crate::manifest::DaemonConfig> {
        None
    }
}

/// Resolve an agent by ID. Searches `manifests_dir` first (if provided),
/// then falls back to hardcoded built-in agents.
pub fn find_agent(id: &str, manifests_dir: Option<&Path>) -> Option<Box<dyn AgentDef>> {
    // Manifest lookup takes priority so users can override built-ins.
    if let Some(dir) = manifests_dir {
        if let Some(manifest) = crate::manifest::find_manifest(dir, id) {
            return Some(Box::new(manifest_agent::ManifestAgentDef::new(manifest)));
        }
    }

    // Hardcoded fallbacks.
    match id {
        "claude-code" => Some(Box::new(claude_code::ClaudeCodeAgent)),
        "opencode" => Some(Box::new(opencode::OpenCodeAgent)),
        _ => None,
    }
}

/// Known built-in agent IDs (does not include manifest-only agents).
pub fn known_agent_ids() -> &'static [&'static str] {
    &["claude-code", "opencode"]
}
