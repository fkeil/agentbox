use super::{AgentDef, AgentError};
use crate::config::{ProviderConfig, ProviderType};
use std::collections::HashMap;

pub struct OpenCodeAgent;

impl AgentDef for OpenCodeAgent {
    fn id(&self) -> &str {
        "opencode"
    }

    fn base_image(&self) -> &str {
        "node:22-slim"
    }

    fn install_command(&self) -> Vec<String> {
        vec![
            "sh".into(),
            "-c".into(),
            "apt-get update -qq && apt-get install -y -qq build-essential python3 2>/dev/null; \
             npm install -g opencode-ai"
                .into(),
        ]
    }

    fn supported_providers(&self) -> &[ProviderType] {
        &[
            ProviderType::Anthropic,
            ProviderType::Openai,
            ProviderType::OpenaiCompatible,
        ]
    }

    fn api_key_env_var(&self, provider_type: &ProviderType) -> Option<&str> {
        match provider_type {
            ProviderType::Anthropic => Some("ANTHROPIC_API_KEY"),
            ProviderType::Openai | ProviderType::OpenaiCompatible => Some("OPENAI_API_KEY"),
        }
    }

    fn base_url_env_var(&self, provider_type: &ProviderType) -> Option<&str> {
        match provider_type {
            ProviderType::OpenaiCompatible | ProviderType::Openai => Some("OPENAI_BASE_URL"),
            ProviderType::Anthropic => None,
        }
    }

    fn config_file_path(&self) -> Option<&str> {
        // OpenCode reads its config from ~/.config/opencode/config.json.
        // Running as root in the container means ~ = /root.
        // OpenCode reads all provider config from env vars (OPENAI_API_KEY,
        // OPENAI_BASE_URL, ANTHROPIC_API_KEY). No config file needed.
        None
    }

    fn render_config(
        &self,
        _provider: &ProviderConfig,
        _resolved_key: Option<&str>,
    ) -> Result<Vec<u8>, AgentError> {
        Ok(Vec::new())
    }

    fn launch_command(&self) -> Vec<String> {
        vec!["opencode".into()]
    }

    fn workdir(&self) -> &str {
        "/workspace"
    }

    fn extra_env(&self, _provider: &ProviderConfig) -> HashMap<String, String> {
        HashMap::new()
    }
}
