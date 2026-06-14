use std::path::PathBuf;

/// A resolved secret value. Debug output is redacted to prevent accidental logging.
pub struct ResolvedSecret(String);

impl std::fmt::Debug for ResolvedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolvedSecret([redacted])")
    }
}

impl ResolvedSecret {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("env var `{name}` is not set")]
    EnvVarNotSet { name: String },
    #[error("keychain lookup failed for `{service}/{account}`: {source}")]
    Keychain {
        service: String,
        account: String,
        source: keyring::Error,
    },
    #[error("secret file `{path}` cannot be read: {source}")]
    FileRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unrecognized auth reference syntax: `{0}`")]
    InvalidSyntax(String),
}

/// Like `resolve_auth` but also accepts bare literal strings (no `${…}` wrapper).
/// Use this for `extra_env` values where the user may write a literal string.
pub fn resolve_value(reference: &str) -> Result<ResolvedSecret, AuthError> {
    let trimmed = reference.trim();
    if trimmed.starts_with("${") || trimmed == "none" {
        resolve_auth(reference)
    } else {
        Ok(ResolvedSecret(trimmed.to_owned()))
    }
}

/// Resolve an auth reference string to a plaintext secret.
///
/// Formats:
///   `none`                        → empty string
///   `${env:NAME}`                 → std::env::var("NAME")
///   `${keychain:service/account}` → OS keychain lookup
///   `${file:./path}`              → read file, trim whitespace
pub fn resolve_auth(reference: &str) -> Result<ResolvedSecret, AuthError> {
    let reference = reference.trim();

    if reference == "none" || reference == "oauth" {
        return Ok(ResolvedSecret(String::new()));
    }

    if let Some(inner) = strip_ref(reference, "env") {
        let val = std::env::var(inner).map_err(|_| AuthError::EnvVarNotSet {
            name: inner.to_owned(),
        })?;
        return Ok(ResolvedSecret(val));
    }

    if let Some(inner) = strip_ref(reference, "keychain") {
        let (service, account) = inner.split_once('/').ok_or_else(|| {
            AuthError::InvalidSyntax(format!(
                "keychain reference must be `${{keychain:service/account}}`, got `{reference}`"
            ))
        })?;
        let entry = keyring::Entry::new(service, account).map_err(|e| AuthError::Keychain {
            service: service.to_owned(),
            account: account.to_owned(),
            source: e,
        })?;
        let secret = entry.get_password().map_err(|e| AuthError::Keychain {
            service: service.to_owned(),
            account: account.to_owned(),
            source: e,
        })?;
        return Ok(ResolvedSecret(secret));
    }

    if let Some(inner) = strip_ref(reference, "file") {
        let path = PathBuf::from(inner);
        let content = std::fs::read_to_string(&path).map_err(|e| AuthError::FileRead {
            path: path.clone(),
            source: e,
        })?;
        return Ok(ResolvedSecret(content.trim().to_owned()));
    }

    Err(AuthError::InvalidSyntax(reference.to_owned()))
}

/// Extract the inner value from `${prefix:inner}`, returning None if the
/// reference doesn't match this prefix.
fn strip_ref<'a>(reference: &'a str, prefix: &str) -> Option<&'a str> {
    let tag = format!("${{{prefix}:");
    reference.strip_prefix(tag.as_str())?.strip_suffix('}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_none() {
        let s = resolve_auth("none").unwrap();
        assert_eq!(s.as_str(), "");
    }

    #[test]
    fn resolve_env_present() {
        std::env::set_var("_AGENTBOX_TEST_KEY", "test-secret");
        let s = resolve_auth("${env:_AGENTBOX_TEST_KEY}").unwrap();
        assert_eq!(s.as_str(), "test-secret");
        std::env::remove_var("_AGENTBOX_TEST_KEY");
    }

    #[test]
    fn resolve_env_missing() {
        std::env::remove_var("_AGENTBOX_MISSING");
        let err = resolve_auth("${env:_AGENTBOX_MISSING}").unwrap_err();
        assert!(matches!(err, AuthError::EnvVarNotSet { .. }));
    }

    #[test]
    fn resolve_invalid_syntax() {
        let err = resolve_auth("plaintext-secret").unwrap_err();
        assert!(matches!(err, AuthError::InvalidSyntax(_)));
    }

    #[test]
    fn oauth_keyword_resolves_to_empty() {
        // `auth: oauth` means the agent handles its own token flow; the engine
        // injects an empty string so the env var is present but blank.
        let s = resolve_auth("oauth").unwrap();
        assert_eq!(s.as_str(), "");
    }
}
