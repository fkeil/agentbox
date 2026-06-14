use crate::config::{ProviderConfig, ProviderType};
use std::collections::HashMap;

pub mod claude_code;
pub mod opencode;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("config rendering failed: {0}")]
    RenderFailed(String),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Everything the engine needs to know about an agent. Implemented by
/// hardcoded structs in Phase 1; Phase 2 will add a ManifestAgentDef that
/// reads the same interface from YAML.
pub trait AgentDef: Send + Sync {
    fn id(&self) -> &str;
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
    /// In-container path where the user folder is bind-mounted.
    fn workdir(&self) -> &str;
    /// Extra env vars the agent always needs beyond auth injection.
    fn extra_env(&self, provider: &ProviderConfig) -> HashMap<String, String>;
}

/// Resolve an agent by its ID. Returns None for unknown IDs.
/// Phase 2 will extend this to also search loaded YAML manifests.
pub fn find_agent(id: &str) -> Option<Box<dyn AgentDef>> {
    match id {
        "claude-code" => Some(Box::new(claude_code::ClaudeCodeAgent)),
        "opencode" => Some(Box::new(opencode::OpenCodeAgent)),
        _ => None,
    }
}

/// All known agent IDs, for error messages.
pub fn known_agent_ids() -> &'static [&'static str] {
    &["claude-code", "opencode"]
}
