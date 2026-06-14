pub use crate::config::{ConfigError, ProviderType};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error(
        "agent `{agent}` does not support provider type `{provider_type}`.\n\
         Supported types: {supported}"
    )]
    IncompatibleProvider {
        agent: String,
        provider_type: String,
        supported: String,
    },
    #[error("`openai-compatible` provider requires `base_url` to be set")]
    MissingBaseUrl,
}

pub fn check_provider_compat(
    agent_id: &str,
    provider_type: &ProviderType,
    supported: &[ProviderType],
) -> Result<(), ProviderError> {
    if supported.contains(provider_type) {
        return Ok(());
    }
    let supported_str = supported
        .iter()
        .map(provider_type_name)
        .collect::<Vec<_>>()
        .join(", ");
    Err(ProviderError::IncompatibleProvider {
        agent: agent_id.to_owned(),
        provider_type: provider_type_name(provider_type).to_owned(),
        supported: supported_str,
    })
}

fn provider_type_name(pt: &ProviderType) -> &'static str {
    match pt {
        ProviderType::Anthropic => "anthropic",
        ProviderType::Openai => "openai",
        ProviderType::OpenaiCompatible => "openai-compatible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_pass() {
        assert!(check_provider_compat(
            "claude-code",
            &ProviderType::Anthropic,
            &[ProviderType::Anthropic]
        )
        .is_ok());
    }

    #[test]
    fn compat_fail() {
        let err = check_provider_compat(
            "claude-code",
            &ProviderType::OpenaiCompatible,
            &[ProviderType::Anthropic],
        )
        .unwrap_err();
        assert!(err.to_string().contains("claude-code"));
    }
}
