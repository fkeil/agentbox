use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agents::{self, AgentError};
use crate::auth::AuthError;
use crate::config::{self, BoxConfig, ConfigError, Lifecycle, ProviderType, SyncMode};
use crate::container::{
    BoxInfo, ContainerBackend, ContainerError, ContainerId, ContainerSpec, ContainerStatus,
    DockerBackend,
};
use crate::provider::{self, ProviderError};

pub use crate::container::BoxInfo as BoxSummary;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Config(#[from] ConfigError),
    #[error("unknown agent `{0}`. Built-ins: claude-code, opencode. Or add a manifests/{0}.yaml file.")]
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

// ── Public entry points ───────────────────────────────────────────────────────

/// Run a box from a config file.
pub async fn run_box(config_path: &Path) -> Result<(), EngineError> {
    let cfg = config::parse_config(config_path)?;
    config::validate_config(&cfg)?;

    let manifests_dir = config_path
        .parent()
        .map(|d| d.join("manifests"))
        .filter(|d| d.is_dir())
        .or_else(|| {
            let cwd = std::env::current_dir().ok()?;
            let d = cwd.join("manifests");
            d.is_dir().then_some(d)
        });

    run_box_config(cfg, manifests_dir.as_deref()).await
}

/// Run a box from a pre-parsed config. Called by the TUI after the wizard
/// collects settings.
pub async fn run_box_config(cfg: BoxConfig, manifests_dir: Option<&Path>) -> Result<(), EngineError> {
    let agent = agents::find_agent(&cfg.agent.0, manifests_dir)
        .ok_or_else(|| EngineError::UnknownAgent(cfg.agent.0.clone()))?;

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

    let auth_ref = cfg.provider.auth.clone();
    let resolved_secret =
        tokio::task::spawn_blocking(move || crate::auth::resolve_auth(&auth_ref)).await??;

    let docker = DockerBackend::connect()?;

    // Build env vars
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
    for (key, val_ref) in &cfg.extra_env {
        let val_ref = val_ref.clone();
        let resolved =
            tokio::task::spawn_blocking(move || crate::auth::resolve_value(&val_ref)).await??;
        env_vars.push(format!("{key}={}", resolved.as_str()));
    }

    let host_folder = cfg
        .folder
        .path
        .canonicalize()
        .map_err(|e| EngineError::BadFolderPath {
            path: cfg.folder.path.clone(),
            source: e,
        })?;
    let memory_limit = cfg
        .resources
        .memory
        .as_deref()
        .map(config::parse_memory_bytes)
        .transpose()?;

    let cache_image = format!("agentbox-cache-{}:latest", agent.id());
    let use_cache = docker.image_exists(&cache_image).await;
    let base_image = if use_cache { cache_image.clone() } else { agent.base_image().to_string() };

    let mut launch_cmd = agent.launch_command();
    launch_cmd.extend(agent.launch_args(&cfg.provider));
    let launch_cmd_json = serde_json::to_string(&launch_cmd).unwrap_or_default();

    if cfg.folder.sync == SyncMode::Snapshot {
        let diffs = run_ephemeral_snapshot(
            &docker,
            &cfg,
            agent.as_ref(),
            env_vars,
            memory_limit,
            base_image,
            cache_image,
            use_cache,
            launch_cmd,
            host_folder.clone(),
            resolved_secret.as_str(),
        )
        .await?;

        crate::sync::store_diff(&diffs, &host_folder).ok();

        if diffs.is_empty() {
            eprintln!("Agent made no changes.");
        } else {
            eprintln!(
                "\n{} file(s) changed. Diff stored at: {}",
                diffs.len(),
                crate::sync::diff_path_for(&host_folder).display()
            );
        }
        return Ok(());
    }

    let bind_mounts = vec![(
        host_folder.to_string_lossy().into_owned(),
        agent.workdir().to_string(),
    )];

    if cfg.lifecycle == Lifecycle::Persistent {
        run_persistent(
            &docker,
            &cfg,
            agent.as_ref(),
            env_vars,
            bind_mounts,
            memory_limit,
            base_image,
            cache_image,
            use_cache,
            launch_cmd,
            launch_cmd_json,
            host_folder.to_string_lossy().into_owned(),
            resolved_secret.as_str(),
        )
        .await
    } else {
        run_ephemeral(
            &docker,
            &cfg,
            agent.as_ref(),
            env_vars,
            bind_mounts,
            memory_limit,
            base_image,
            cache_image,
            use_cache,
            launch_cmd,
            host_folder,
            resolved_secret.as_str(),
        )
        .await
    }
}

/// Attach to an existing persistent box by name.
pub async fn attach_box(box_name: &str) -> Result<(), EngineError> {
    let docker = DockerBackend::connect()?;
    let container_name = format!("agentbox-{box_name}");
    let id = ContainerId(container_name.clone());

    let labels = docker
        .get_container_labels(&id)
        .await
        .map_err(|_| ContainerError::BoxNotFound(box_name.to_string()))?;

    let workdir = labels
        .get("agentbox.workdir")
        .map(|s| s.as_str())
        .unwrap_or("/workspace");
    let launch_cmd_json = labels
        .get("agentbox.launch-cmd")
        .cloned()
        .unwrap_or_else(|| r#"["sh"]"#.to_string());
    let launch_cmd: Vec<String> =
        serde_json::from_str(&launch_cmd_json).unwrap_or_else(|_| vec!["sh".into()]);

    docker.start_container(&id).await.ok(); // no-op if already running

    eprintln!("Attaching to box '{box_name}'...");
    let exit_code = docker
        .attach_interactive(&id, &launch_cmd, workdir)
        .await?;

    docker.stop_container(&id).await.ok();
    eprintln!("Box stopped. State preserved.");

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }
    Ok(())
}

/// Stop a running persistent box without removing it.
pub async fn stop_box(box_name: &str) -> Result<(), EngineError> {
    let docker = DockerBackend::connect()?;
    docker
        .stop_container(&ContainerId(format!("agentbox-{box_name}")))
        .await?;
    Ok(())
}

/// Remove a persistent box: stop + remove container + remove state volume.
pub async fn remove_box(box_name: &str) -> Result<(), EngineError> {
    let docker = DockerBackend::connect()?;
    let container_name = format!("agentbox-{box_name}");
    let state_volume = format!("agentbox-state-{box_name}");
    docker
        .stop_container(&ContainerId(container_name.clone()))
        .await
        .ok();
    docker
        .remove_container(&ContainerId(container_name))
        .await
        .ok();
    docker.remove_volume(&state_volume).await.ok();
    eprintln!("Box '{box_name}' removed.");
    Ok(())
}

/// List all agentbox-managed boxes.
pub async fn list_boxes() -> Result<Vec<BoxInfo>, EngineError> {
    let docker = DockerBackend::connect()?;
    Ok(docker.list_boxes().await?)
}

/// Stop and remove a named box (alias for remove_box for CLI compatibility).
pub async fn down_box(box_name: &str) -> Result<(), EngineError> {
    remove_box(box_name).await
}

/// Apply changes from the last snapshot run for `host_folder`.
/// Only files named in `approved_paths` are written back to the host.
pub async fn apply_snapshot_diff(
    host_folder: &std::path::Path,
    approved_paths: &[String],
) -> Result<(), EngineError> {
    let diffs = crate::sync::load_diff(host_folder)
        .ok_or_else(|| EngineError::Container(ContainerError::BoxNotFound(
            format!("no snapshot diff found for {}", host_folder.display()),
        )))?;
    crate::sync::apply_approved_changes(host_folder, &diffs, approved_paths)
        .map_err(|e| EngineError::Container(ContainerError::Io(e)))
}

// ── Lifecycle implementations ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_ephemeral_snapshot(
    docker: &DockerBackend,
    cfg: &BoxConfig,
    agent: &dyn crate::agents::AgentDef,
    env_vars: Vec<String>,
    memory_limit: Option<u64>,
    base_image: String,
    cache_image: String,
    use_cache: bool,
    launch_cmd: Vec<String>,
    host_folder: std::path::PathBuf,
    resolved_key: &str,
) -> Result<Vec<crate::sync::FileDiff>, EngineError> {
    let container_name = format!(
        "agentbox-{}-{}",
        agent.id(),
        slug_from_path(&host_folder)
    );

    docker
        .remove_container(&ContainerId(container_name.clone()))
        .await
        .ok();

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts: vec![],  // no bind mount — folder is copied in
        volume_mounts: vec![],
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels: HashMap::new(),
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: false,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    eprintln!("Copying workspace into container (snapshot mode)...");
    docker
        .copy_dir_to_container(&container_id, &host_folder, agent.workdir())
        .await?;

    eprintln!("Launching {}...", agent.id());
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir())
        .await?;

    eprintln!("Computing diff...");
    let diffs = crate::sync::compute_snapshot_diff(
        docker,
        &container_id,
        agent.workdir(),
        &host_folder,
    )
    .await?;

    drop(cleanup);

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }
    Ok(diffs)
}

#[allow(clippy::too_many_arguments)]
async fn run_ephemeral(
    docker: &DockerBackend,
    cfg: &BoxConfig,
    agent: &dyn crate::agents::AgentDef,
    env_vars: Vec<String>,
    bind_mounts: Vec<(String, String)>,
    memory_limit: Option<u64>,
    base_image: String,
    cache_image: String,
    use_cache: bool,
    launch_cmd: Vec<String>,
    host_folder: std::path::PathBuf,
    resolved_key: &str,
) -> Result<(), EngineError> {
    let container_name = format!(
        "agentbox-{}-{}",
        agent.id(),
        slug_from_path(&host_folder)
    );

    docker
        .remove_container(&ContainerId(container_name.clone()))
        .await
        .ok();

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts,
        volume_mounts: vec![],
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels: HashMap::new(),
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: false,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    eprintln!("Launching {}...", agent.id());
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir())
        .await?;

    drop(cleanup);

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_persistent(
    docker: &DockerBackend,
    cfg: &BoxConfig,
    agent: &dyn crate::agents::AgentDef,
    env_vars: Vec<String>,
    bind_mounts: Vec<(String, String)>,
    memory_limit: Option<u64>,
    base_image: String,
    cache_image: String,
    use_cache: bool,
    launch_cmd: Vec<String>,
    launch_cmd_json: String,
    host_folder_str: String,
    resolved_key: &str,
) -> Result<(), EngineError> {
    let box_name = cfg.name.as_deref().unwrap(); // validated earlier
    let container_name = format!("agentbox-{box_name}");
    let state_volume = format!("agentbox-state-{box_name}");
    let id = ContainerId(container_name.clone());

    // Check if this box already exists.
    if let Some(status) = docker.container_status(&container_name).await {
        if status == ContainerStatus::Stopped {
            docker.start_container(&id).await.ok();
        }
        // Always refresh the agent config in case provider settings changed.
        write_agent_config(docker, &id, agent, &cfg.provider, resolved_key).await?;
        eprintln!("Reconnecting to box '{box_name}'...");
        let exit_code = docker
            .attach_interactive(&id, &launch_cmd, agent.workdir())
            .await?;
        docker.stop_container(&id).await.ok();
        eprintln!("Box stopped. State preserved.");
        if exit_code != 0 {
            eprintln!("Agent exited with code {exit_code}");
        }
        return Ok(());
    }

    // New persistent box — create state volume and container.
    if !docker.volume_exists(&state_volume).await {
        docker.create_volume(&state_volume).await?;
    }

    let mut labels = HashMap::new();
    labels.insert("agentbox.managed".into(), "true".into());
    labels.insert("agentbox.box-name".into(), box_name.to_string());
    labels.insert("agentbox.agent-id".into(), agent.id().to_string());
    labels.insert(
        "agentbox.agent-display-name".into(),
        agent.id().to_string(),
    );
    labels.insert("agentbox.workdir".into(), agent.workdir().to_string());
    labels.insert("agentbox.launch-cmd".into(), launch_cmd_json);
    labels.insert("agentbox.folder".into(), host_folder_str);

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts,
        // Mount state volume at /root so history, credentials, and config persist.
        volume_mounts: vec![(state_volume, "/root".to_string())],
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels,
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: true,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    eprintln!("Launching {}...", agent.id());
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir())
        .await?;

    drop(cleanup);

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }
    Ok(())
}

// ── Shared install + config helpers ──────────────────────────────────────────

async fn install_and_cache(
    docker: &DockerBackend,
    id: &ContainerId,
    agent: &dyn crate::agents::AgentDef,
    cache_image: &str,
    use_cache: bool,
) -> Result<(), EngineError> {
    if use_cache {
        eprintln!("Using cached {} image.", agent.id());
    } else {
        eprint!("Installing {}... ", agent.id());
        let result = docker
            .exec_command(id, &agent.install_command(), &[])
            .await?;
        if result.exit_code != 0 {
            eprintln!("failed.");
            return Err(EngineError::Container(ContainerError::InstallFailed {
                code: result.exit_code,
                stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            }));
        }
        eprintln!("done.");
        eprint!("Caching image... ");
        match docker.commit_container(id, cache_image).await {
            Ok(()) => eprintln!("done."),
            Err(e) => eprintln!("warning: could not cache image: {e}"),
        }
    }
    Ok(())
}

async fn write_agent_config(
    docker: &DockerBackend,
    id: &ContainerId,
    agent: &dyn crate::agents::AgentDef,
    provider: &crate::config::ProviderConfig,
    resolved_key: &str,
) -> Result<(), EngineError> {
    if let Some(cfg_path) = agent.config_file_path() {
        let cfg_bytes = agent.render_config(
            provider,
            if provider.auth != "none" { Some(resolved_key) } else { None },
        )?;
        docker.write_file(id, cfg_path, &cfg_bytes).await?;
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn host_gateway_hosts() -> Vec<String> {
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

/// RAII guard: stops the container on drop; also removes it unless the box is persistent.
struct CleanupGuard<'a> {
    docker: &'a DockerBackend,
    id: ContainerId,
    persistent: bool,
}

impl Drop for CleanupGuard<'_> {
    fn drop(&mut self) {
        tokio::task::block_in_place(|| {
            let handle = tokio::runtime::Handle::current();
            handle.block_on(async {
                let _ = self.docker.stop_container(&self.id).await;
                if !self.persistent {
                    let _ = self.docker.remove_container(&self.id).await;
                    eprintln!("Container removed.");
                } else {
                    eprintln!("Box stopped. Run `agentbox up` or `agentbox-tui` to reconnect.");
                }
            });
        });
    }
}
