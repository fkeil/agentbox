use super::{AgentDef, AgentError};
use crate::agents::claude_code::merge_raw;
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
        Some("/root/.config/opencode/config.json")
    }

    fn render_config(
        &self,
        provider: &ProviderConfig,
        _resolved_key: Option<&str>,
    ) -> Result<Vec<u8>, AgentError> {
        let mut cfg = serde_json::json!({
            "model": provider.model,
        });

        if let Some(base_url) = &provider.base_url {
            cfg["baseURL"] = serde_json::Value::String(base_url.clone());
        }

        merge_raw(&mut cfg, &provider.raw);

        Ok(serde_json::to_vec_pretty(&cfg)?)
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
