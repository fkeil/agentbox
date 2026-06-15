use std::collections::HashMap;

use crate::agents::{AgentDef, AgentError};
use crate::config::{ProviderConfig, ProviderType};
use crate::manifest::{AgentManifest, ConfigSource};

pub struct ManifestAgentDef {
    manifest: AgentManifest,
}

impl ManifestAgentDef {
    pub fn new(manifest: AgentManifest) -> Self {
        Self { manifest }
    }
}

impl AgentDef for ManifestAgentDef {
    fn id(&self) -> &str {
        &self.manifest.id
    }

    fn display_name(&self) -> &str {
        &self.manifest.display_name
    }

    fn base_image(&self) -> &str {
        &self.manifest.base_image
    }

    fn install_command(&self) -> Vec<String> {
        self.manifest.install.build_command()
    }

    fn supported_providers(&self) -> &[ProviderType] {
        &self.manifest.supported_providers
    }

    fn api_key_env_var(&self, provider_type: &ProviderType) -> Option<&str> {
        self.manifest
            .auth
            .get(provider_type)
            .and_then(|a| a.api_key_env.as_deref())
    }

    fn base_url_env_var(&self, provider_type: &ProviderType) -> Option<&str> {
        self.manifest
            .auth
            .get(provider_type)
            .and_then(|a| a.base_url_env.as_deref())
    }

    fn config_file_path(&self) -> Option<&str> {
        self.manifest.config.as_ref().map(|c| c.path.as_str())
    }

    fn render_config(
        &self,
        provider: &ProviderConfig,
        _resolved_key: Option<&str>,
    ) -> Result<Vec<u8>, AgentError> {
        let Some(config_def) = &self.manifest.config else {
            return Ok(Vec::new());
        };

        if config_def.source == ConfigSource::Raw {
            // Write provider.raw directly as JSON; empty vec = skip writing.
            return if provider.raw.is_null() {
                Ok(Vec::new())
            } else {
                Ok(serde_json::to_vec_pretty(&provider.raw)?)
            };
        }

        let template = config_def
            .template_for(&provider.provider_type)
            .ok_or_else(|| {
                AgentError::RenderFailed(format!(
                    "manifest `{}` has no config template for provider type `{}`",
                    self.manifest.id,
                    provider_type_str(&provider.provider_type)
                ))
            })?;

        let rendered = render_template(template, provider);

        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&rendered) {
            Ok(serde_json::to_vec_pretty(&v)?)
        } else {
            Ok(rendered.into_bytes())
        }
    }

    fn launch_command(&self) -> Vec<String> {
        self.manifest.launch.command.clone()
    }

    fn launch_args(&self, provider: &ProviderConfig) -> Vec<String> {
        let args = self
            .manifest
            .launch
            .args_by_provider_type
            .get(&provider.provider_type)
            .cloned()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| self.manifest.launch.args.clone());

        args.into_iter()
            .map(|arg| render_template(&arg, provider))
            .collect()
    }

    fn workdir(&self) -> &str {
        &self.manifest.workdir
    }

    fn extra_env(&self, _provider: &ProviderConfig) -> HashMap<String, String> {
        self.manifest.env.clone()
    }

    fn healthcheck_command(&self) -> Option<Vec<String>> {
        self.manifest.healthcheck.clone()
    }

    fn oauth_cache_path(&self) -> Option<&str> {
        self.manifest
            .oauth
            .as_ref()
            .filter(|o| o.supported)
            .and_then(|o| o.cache_path.as_deref())
    }

    fn daemon_config(&self) -> Option<&crate::manifest::DaemonConfig> {
        self.manifest.daemon.as_ref()
    }

    fn cost_config(&self) -> Option<&crate::manifest::CostConfig> {
        self.manifest.cost.as_ref()
    }
}

/// Substitute `{{var}}` placeholders in a template string.
fn render_template(template: &str, provider: &ProviderConfig) -> String {
    let slug = provider_slug(&provider.name);
    template
        .replace("{{model}}", &provider.model)
        .replace("{{base_url}}", provider.base_url.as_deref().unwrap_or(""))
        .replace(
            "{{provider_type}}",
            provider_type_str(&provider.provider_type),
        )
        .replace("{{provider_name}}", &provider.name)
        .replace("{{provider_slug}}", &slug)
}

fn provider_type_str(t: &ProviderType) -> &'static str {
    match t {
        ProviderType::Anthropic => "anthropic",
        ProviderType::Openai => "openai",
        ProviderType::OpenaiCompatible => "openai-compatible",
    }
}

/// Slugify a provider name: lowercase, non-alphanumeric → `-`, collapse runs.
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
