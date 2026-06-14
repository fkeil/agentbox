use super::{AgentDef, AgentError};
use crate::config::{ProviderConfig, ProviderType};
use std::collections::HashMap;

pub struct ClaudeCodeAgent;

impl AgentDef for ClaudeCodeAgent {
    fn id(&self) -> &str {
        "claude-code"
    }

    fn base_image(&self) -> &str {
        "node:22-slim"
    }

    fn install_command(&self) -> Vec<String> {
        // Use sh -c so we can chain apt-get + npm in a single exec call.
        // build-essential and python3 are needed for npm packages with native
        // addons; installing them defensively avoids a confusing failure later.
        vec![
            "sh".into(),
            "-c".into(),
            "apt-get update -qq && apt-get install -y -qq build-essential python3 2>/dev/null; \
             npm install -g @anthropic-ai/claude-code"
                .into(),
        ]
    }

    fn supported_providers(&self) -> &[ProviderType] {
        &[ProviderType::Anthropic]
    }

    fn api_key_env_var(&self, _provider_type: &ProviderType) -> Option<&str> {
        Some("ANTHROPIC_API_KEY")
    }

    fn base_url_env_var(&self, _provider_type: &ProviderType) -> Option<&str> {
        Some("ANTHROPIC_BASE_URL")
    }

    fn config_file_path(&self) -> Option<&str> {
        Some("/root/.claude/settings.json")
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
            cfg["env"] = serde_json::json!({ "ANTHROPIC_BASE_URL": base_url });
        }

        merge_raw(&mut cfg, &provider.raw);

        Ok(serde_json::to_vec_pretty(&cfg)?)
    }

    fn launch_command(&self) -> Vec<String> {
        vec!["claude".into()]
    }

    fn workdir(&self) -> &str {
        "/workspace"
    }

    fn extra_env(&self, _provider: &ProviderConfig) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// Merge `raw` JSON object fields into `target`, with raw values winning.
pub(crate) fn merge_raw(target: &mut serde_json::Value, raw: &serde_json::Value) {
    if let (serde_json::Value::Object(t), serde_json::Value::Object(r)) = (target, raw) {
        for (k, v) in r {
            t.insert(k.clone(), v.clone());
        }
    }
}
