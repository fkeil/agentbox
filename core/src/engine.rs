use std::path::{Path, PathBuf};

use crate::agents::{self, AgentError};
use crate::auth::AuthError;
use crate::config::{self, ConfigError, ProviderType};
use crate::container::{ContainerBackend, ContainerError, ContainerId, ContainerSpec, DockerBackend};
use crate::provider::{self, ProviderError};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("unknown agent `{0}`. Known agents: claude-code, opencode")]
    UnknownAgent(String),
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("{0}")]
    Auth(#[from] AuthError),
    #[error("{0}")]
    Container(#[from] ContainerError),
    #[error("{0}")]
    AgentConfig(#[from] AgentError),
    #[error("cannot resolve folder path `{path}`: {source}")]
    BadFolderPath { path: PathBuf, source: std::io::Error },
    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
}

/// Run a box from a config file. Blocks until the agent exits, then tears
/// down the container (ephemeral lifecycle).
pub async fn run_box(config_path: &Path) -> Result<(), EngineError> {
    // --- 1. Parse + validate ---
    let cfg = config::parse_config(config_path)?;
    config::validate_config(&cfg)?;

    // --- 2. Resolve agent ---
    let agent = agents::find_agent(&cfg.agent.0)
        .ok_or_else(|| EngineError::UnknownAgent(cfg.agent.0.clone()))?;

    // --- 3. Provider compatibility ---
    provider::check_provider_compat(
        agent.id(),
        &cfg.provider.provider_type,
        agent.supported_providers(),
    )?;

    if cfg.provider.provider_type == ProviderType::OpenaiCompatible
        && cfg.provider.base_url.is_none()
    {
        return Err(EngineError::Provider(ProviderError::MissingBaseUrl));
    }

    // --- 4. Resolve auth ---
    let auth_ref = cfg.provider.auth.clone();
    let resolved_secret = tokio::task::spawn_blocking(move || crate::auth::resolve_auth(&auth_ref))
        .await??;

    // --- 5. Connect to Docker ---
    let docker = DockerBackend::connect()?;

    // --- 6. Build env vars ---
    let mut env_vars: Vec<String> = Vec::new();

    if cfg.provider.auth != "none" {
        if let Some(key_env) = agent.api_key_env_var(&cfg.provider.provider_type) {
            env_vars.push(format!("{}={}", key_env, resolved_secret.as_str()));
        }
    }

    if let Some(base_url) = &cfg.provider.base_url {
        if let Some(url_env) = agent.base_url_env_var(&cfg.provider.provider_type) {
            env_vars.push(format!("{}={}", url_env, base_url));
        }
    }

    for (k, v) in agent.extra_env(&cfg.provider) {
        env_vars.push(format!("{k}={v}"));
    }

    // --- 7. Resolve folder path + build mounts ---
    let host_folder = cfg
        .folder
        .path
        .canonicalize()
        .map_err(|e| EngineError::BadFolderPath {
            path: cfg.folder.path.clone(),
            source: e,
        })?;
    let bind_mounts = vec![(
        host_folder.to_string_lossy().into_owned(),
        agent.workdir().to_string(),
    )];

    // --- 8. Memory limit ---
    let memory_limit = cfg
        .resources
        .memory
        .as_deref()
        .map(config::parse_memory_bytes)
        .transpose()?;

    // --- 9. Container name (deterministic) ---
    let container_name = format!("agentbox-{}-{}", agent.id(), slug_from_path(&host_folder));

    // Remove any leftover container with this name (e.g. from a previous crash).
    let _ = docker.remove_container(&ContainerId(container_name.clone())).await;

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: agent.base_image().to_string(),
        bind_mounts,
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
    };

    // --- 10. Create + start container ---
    eprintln!("Creating container `{container_name}`...");
    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    // Ensure we clean up on any exit path.
    let cleanup = CleanupGuard {
        docker: &docker,
        id: container_id.clone(),
    };

    // --- 11. Install agent ---
    eprintln!("Installing {} (this may take a minute)...", agent.id());
    let result = docker
        .exec_command(&container_id, &agent.install_command(), &[])
        .await?;
    if result.exit_code != 0 {
        return Err(EngineError::Container(ContainerError::InstallFailed {
            code: result.exit_code,
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        }));
    }

    // --- 12. Write native config ---
    if let Some(cfg_path) = agent.config_file_path() {
        let cfg_bytes = agent.render_config(
            &cfg.provider,
            if cfg.provider.auth != "none" {
                Some(resolved_secret.as_str())
            } else {
                None
            },
        )?;
        docker.write_file(&container_id, cfg_path, &cfg_bytes).await?;
    }

    // --- 13. Launch agent interactively ---
    eprintln!("Launching {}...", agent.id());
    let mut launch_cmd = agent.launch_command();
    launch_cmd.extend(agent.launch_args(&cfg.provider));
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir())
        .await?;

    // cleanup runs here via drop
    drop(cleanup);

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }

    Ok(())
}

/// Stop and remove a named box.
/// In Phase 1 (ephemeral only) this is a no-op: containers self-destruct on agent exit.
pub async fn down_box(box_name: &str) -> Result<(), EngineError> {
    eprintln!(
        "Note: in Phase 1 all boxes are ephemeral and self-destruct when the agent exits.\n\
         `{box_name}` is not a known persistent box."
    );
    Ok(())
}

// --- helpers ---

fn host_gateway_hosts() -> Vec<String> {
    // Docker Desktop injects host.docker.internal on macOS/Windows.
    // On Linux we must add it manually.
    #[cfg(target_os = "linux")]
    {
        vec!["host.docker.internal:host-gateway".to_string()]
    }
    #[cfg(not(target_os = "linux"))]
    {
        vec![]
    }
}

fn slug_from_path(p: &Path) -> String {
    p.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
        .chars()
        .take(40)
        .collect()
}

/// RAII guard that stops and removes the container on drop (best-effort).
struct CleanupGuard<'a> {
    docker: &'a DockerBackend,
    id: crate::container::ContainerId,
}

impl Drop for CleanupGuard<'_> {
    fn drop(&mut self) {
        // block_in_place lets us run async code from Drop without creating a
        // nested runtime (which would panic inside the existing tokio runtime).
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                let _ = self.docker.stop_container(&self.id).await;
                let _ = self.docker.remove_container(&self.id).await;
            });
        });
        eprintln!("Container removed.");
    }
}
