use super::{AgentDef, AgentError};
use crate::agents::claude_code::merge_raw;
use crate::config::{ProviderConfig, ProviderType};
use std::collections::HashMap;

pub struct OpenCodeAgent;

impl AgentDef for OpenCodeAgent {
    fn id(&self) -> &str {
        "opencode"
    }
    fn display_name(&self) -> &str {
        "OpenCode"
    }

    fn base_image(&self) -> &str {
        "node:22-slim"
    }

    fn install_command(&self) -> Vec<String> {
        vec![
            "sh".into(),
            "-c".into(),
            // @ai-sdk/openai-compatible is required by any custom provider using
            // the openai-compatible adapter — it is not bundled with opencode-ai.
            "apt-get update -qq && apt-get install -y -qq build-essential python3 2>/dev/null; \
             npm install -g opencode-ai @ai-sdk/openai-compatible"
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
        let mut cfg = match &provider.provider_type {
            ProviderType::OpenaiCompatible => {
                // Custom provider using the openai-compatible AI SDK adapter.
                // The provider key is derived from provider.name in box.yaml.
                let key = provider_slug(&provider.name);
                let mut provider_obj = serde_json::json!({
                    "npm": "@ai-sdk/openai-compatible",
                    "name": &provider.name,
                    "models": { &provider.model: { "name": &provider.model } },
                });
                if let Some(base_url) = &provider.base_url {
                    provider_obj["options"] = serde_json::json!({ "baseURL": base_url });
                }
                serde_json::json!({
                    "$schema": "https://opencode.ai/config.json",
                    "provider": { &key: provider_obj },
                    "model": format!("{}/{}", key, provider.model),
                })
            }
            ProviderType::Openai => {
                // Built-in openai provider.
                serde_json::json!({
                    "$schema": "https://opencode.ai/config.json",
                    "model": format!("openai/{}", provider.model),
                })
            }
            ProviderType::Anthropic => {
                // Built-in anthropic provider.
                serde_json::json!({
                    "$schema": "https://opencode.ai/config.json",
                    "model": format!("anthropic/{}", provider.model),
                })
            }
        };

        merge_raw(&mut cfg, &provider.raw);
        Ok(serde_json::to_vec_pretty(&cfg)?)
    }

    fn launch_command(&self) -> Vec<String> {
        vec!["opencode".into()]
    }

    fn launch_args(&self, provider: &ProviderConfig) -> Vec<String> {
        let key = provider_slug(&provider.name);
        let model_ref = match &provider.provider_type {
            ProviderType::OpenaiCompatible => format!("{}/{}", key, provider.model),
            ProviderType::Openai => format!("openai/{}", provider.model),
            ProviderType::Anthropic => format!("anthropic/{}", provider.model),
        };
        vec!["-m".into(), model_ref]
    }

    fn workdir(&self) -> &str {
        "/workspace"
    }

    fn extra_env(&self, _provider: &ProviderConfig) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// Turn a provider name like "Ollama (local)" into a valid config key like "ollama-local".
fn provider_slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProviderConfig, ProviderType};

    fn make_provider(type_: ProviderType, model: &str, base_url: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            name: "local-ollama".into(),
            provider_type: type_,
            model: model.into(),
            base_url: base_url.map(String::from),
            auth: "none".into(),
            raw: serde_json::Value::Null,
        }
    }

    #[test]
    fn renders_openai_compatible_config() {
        let agent = OpenCodeAgent;
        let provider = make_provider(
            ProviderType::OpenaiCompatible,
            "gemma4:latest",
            Some("http://192.168.1.4:30068/v1"),
        );
        let bytes = agent.render_config(&provider, None).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(v["model"], "local-ollama/gemma4:latest");
        assert_eq!(
            v["provider"]["local-ollama"]["npm"],
            "@ai-sdk/openai-compatible"
        );
        assert_eq!(
            v["provider"]["local-ollama"]["options"]["baseURL"],
            "http://192.168.1.4:30068/v1"
        );
        assert_eq!(
            v["provider"]["local-ollama"]["models"]["gemma4:latest"]["name"],
            "gemma4:latest"
        );
    }

    #[test]
    fn renders_anthropic_config() {
        let agent = OpenCodeAgent;
        let provider = make_provider(ProviderType::Anthropic, "claude-sonnet-4-6", None);
        let bytes = agent.render_config(&provider, None).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["model"], "anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn provider_slug_examples() {
        assert_eq!(provider_slug("local-ollama"), "local-ollama");
        assert_eq!(provider_slug("Ollama (local)"), "ollama-local");
        assert_eq!(provider_slug("My  Provider"), "my-provider");
    }
}
