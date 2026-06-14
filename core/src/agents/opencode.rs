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

    fn base_url_env_var(&self, _provider_type: &ProviderType) -> Option<&str> {
        // base_url goes into the config file (provider.openai.api), not an env var.
        None
    }

    fn config_file_path(&self) -> Option<&str> {
        Some("/root/.config/opencode/config.json")
    }

    fn render_config(
        &self,
        provider: &ProviderConfig,
        _resolved_key: Option<&str>,
    ) -> Result<Vec<u8>, AgentError> {
        // OpenCode uses "openai" as the provider key for both openai and
        // openai-compatible providers. Anthropic gets its own key.
        let (provider_key, model_prefix) = match &provider.provider_type {
            ProviderType::Anthropic => ("anthropic", "anthropic"),
            ProviderType::Openai | ProviderType::OpenaiCompatible => ("openai", "openai"),
        };

        // Provider object: custom API base URL + model registered in the models map.
        let mut provider_obj = serde_json::json!({
            "models": {
                &provider.model: { "name": &provider.model }
            }
        });
        if let Some(base_url) = &provider.base_url {
            provider_obj["api"] = serde_json::Value::String(base_url.clone());
        }

        let mut cfg = serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
            "provider": { provider_key: provider_obj },
            "model": format!("{}/{}", model_prefix, provider.model),
        });

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
