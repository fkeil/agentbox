use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agents::{self, AgentError};
use crate::auth::AuthError;
use crate::config::{self, BoxConfig, ConfigError, Lifecycle, NetworkMode, ProviderType, SyncMode};
use crate::container::{
    BoxInfo, ContainerBackend, ContainerError, ContainerId, ContainerSpec, ContainerStatus,
    DockerBackend,
};
use crate::provider::{self, ProviderError};

pub use crate::container::{BoxInfo as BoxSummary, CacheImage};

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
    #[error("agent `{0}` does not support in-container OAuth; use `auth: ${{env:API_KEY_VAR}}` instead")]
    OAuthNotSupported(String),
    #[error("healthcheck failed for agent `{agent}` (exit {code}):\n{stderr}")]
    HealthcheckFailed { agent: String, code: i64, stderr: String },
    #[error("egress allowlist setup failed: {0}")]
    AllowlistSetup(String),
    #[error("daemon agent `{0}` requires `lifecycle: persistent`; set `lifecycle: persistent` and `name: <box-name>` in box.yaml")]
    DaemonRequiresPersistent(String),
    #[error("daemon setup failed (exit={code}):\n{stderr}")]
    DaemonSetupFailed { code: i64, stderr: String },
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
        })
        .or_else(find_manifests_near_exe);

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

    // OAuth: validate + prepare credential-cache volume.
    let oauth_volume: Option<(String, String)> = if cfg.provider.auth == "oauth" {
        let cache_path = agent
            .oauth_cache_path()
            .ok_or_else(|| EngineError::OAuthNotSupported(agent.id().to_string()))?
            .to_string();
        let vol_name = format!("agentbox-oauth-{}", agent.id());
        docker.create_volume(&vol_name).await?;
        eprintln!("OAuth credential cache: volume {vol_name} → {cache_path}");
        Some((vol_name, cache_path))
    } else {
        None
    };

    // Build env vars
    let mut env_vars: Vec<String> = Vec::new();
    let inject_key = cfg.provider.auth != "none" && cfg.provider.auth != "oauth";
    if inject_key {
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
        // Record host file metadata before the session for conflict detection.
        let pre_meta = crate::sync::snapshot_host_meta(&host_folder);
        crate::sync::store_snapshot_meta(&pre_meta, &host_folder).ok();

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
            oauth_volume.as_ref(),
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

    // Daemon agents bypass the normal lifecycle branches.
    if let Some(daemon_cfg) = agent.daemon_config() {
        if cfg.lifecycle != Lifecycle::Persistent {
            return Err(EngineError::DaemonRequiresPersistent(agent.id().to_string()));
        }
        return run_daemon(
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
            oauth_volume.as_ref(),
            daemon_cfg,
        )
        .await;
    }

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
            oauth_volume.as_ref(),
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
            oauth_volume.as_ref(),
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

/// Force-stop and remove a container by box name (no state-volume cleanup).
/// Works for both persistent and ephemeral containers. For persistent boxes,
/// the state volume is left intact so the box can be recreated.
pub async fn kill_box(box_name: &str) -> Result<(), EngineError> {
    let docker = DockerBackend::connect()?;
    let container_name = format!("agentbox-{box_name}");
    docker
        .stop_container(&ContainerId(container_name.clone()))
        .await
        .ok();
    docker
        .remove_container(&ContainerId(container_name.clone()))
        .await
        .ok();
    eprintln!("Container '{container_name}' removed.");
    Ok(())
}

/// List all cached agent install images (`agentbox-cache-*:latest`).
pub async fn list_cache_images() -> Result<Vec<CacheImage>, EngineError> {
    let docker = DockerBackend::connect()?;
    Ok(docker.list_cache_images().await?)
}

/// Remove a specific agent's cache image so it is rebuilt on next launch.
pub async fn remove_cache_image(agent_id: &str) -> Result<(), EngineError> {
    let docker = DockerBackend::connect()?;
    let image_name = format!("agentbox-cache-{agent_id}:latest");
    docker.remove_image(&image_name).await?;
    eprintln!("Cache image '{image_name}' removed.");
    Ok(())
}

/// Apply changes from the last snapshot run for `host_folder`.
/// Only files named in `approved_paths` are written back to the host.
pub async fn apply_snapshot_diff(
    host_folder: &std::path::Path,
    approved_paths: &[String],
) -> Result<(), EngineError> {
    // Warn about files that changed on the host since the snapshot was taken.
    if let Some(pre_meta) = crate::sync::load_snapshot_meta(host_folder) {
        let conflicts = crate::sync::detect_conflicts(host_folder, approved_paths, &pre_meta);
        if !conflicts.is_empty() {
            eprintln!("Warning: the following files changed on the host during the session:");
            for p in &conflicts {
                eprintln!("  ! {p}");
            }
            eprintln!("Applying agent changes anyway — review carefully.");
        }
    }

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
    oauth_volume: Option<&(String, String)>,
) -> Result<Vec<crate::sync::FileDiff>, EngineError> {
    let container_name = format!(
        "agentbox-{}-{}",
        agent.id(),
        slug_from_basename(&host_folder)
    );

    docker
        .remove_container(&ContainerId(container_name.clone()))
        .await
        .ok();

    let mut volume_mounts: Vec<(String, String)> = vec![];
    if let Some((vol, path)) = oauth_volume {
        volume_mounts.push((vol.clone(), path.clone()));
    }

    let box_slug = format!("{}-{}", agent.id(), slug_from_basename(&host_folder));
    let mut labels = HashMap::new();
    labels.insert("agentbox.managed".into(), "true".into());
    labels.insert("agentbox.lifecycle".into(), "ephemeral".into());
    labels.insert("agentbox.box-name".into(), box_slug);
    labels.insert("agentbox.agent-id".into(), agent.id().to_string());
    labels.insert("agentbox.agent-display-name".into(), agent.display_name().to_string());
    labels.insert("agentbox.folder".into(), host_folder.to_string_lossy().into_owned());
    if let Some(pn) = &cfg.project_name {
        labels.insert("agentbox.project-name".into(), pn.clone());
    }

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts: vec![],
        volume_mounts,
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels,
        cap_add: allowlist_caps(cfg),
        port_bindings: vec![],
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: false,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    run_healthcheck(docker, &container_id, agent).await?;
    apply_egress_allowlist(docker, &container_id, cfg).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    eprintln!("Copying workspace into container (snapshot mode)...");
    docker
        .copy_dir_to_container(&container_id, &host_folder, agent.workdir())
        .await?;

    let project = resolve_project_name(cfg.project_name.as_deref(), &host_folder);
    eprintln!("Launching {}...", box_label(agent.display_name(), &project));
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
    oauth_volume: Option<&(String, String)>,
) -> Result<(), EngineError> {
    let container_name = format!(
        "agentbox-{}-{}",
        agent.id(),
        slug_from_basename(&host_folder)
    );

    docker
        .remove_container(&ContainerId(container_name.clone()))
        .await
        .ok();

    let mut volume_mounts: Vec<(String, String)> = vec![];
    if let Some((vol, path)) = oauth_volume {
        volume_mounts.push((vol.clone(), path.clone()));
    }

    let box_slug = format!("{}-{}", agent.id(), slug_from_basename(&host_folder));
    let mut labels = HashMap::new();
    labels.insert("agentbox.managed".into(), "true".into());
    labels.insert("agentbox.lifecycle".into(), "ephemeral".into());
    labels.insert("agentbox.box-name".into(), box_slug);
    labels.insert("agentbox.agent-id".into(), agent.id().to_string());
    labels.insert("agentbox.agent-display-name".into(), agent.display_name().to_string());
    labels.insert("agentbox.folder".into(), host_folder.to_string_lossy().into_owned());
    if let Some(pn) = &cfg.project_name {
        labels.insert("agentbox.project-name".into(), pn.clone());
    }

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts,
        volume_mounts,
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels,
        cap_add: allowlist_caps(cfg),
        port_bindings: vec![],
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: false,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    run_healthcheck(docker, &container_id, agent).await?;
    apply_egress_allowlist(docker, &container_id, cfg).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    let project = resolve_project_name(cfg.project_name.as_deref(), &host_folder);
    eprintln!("Launching {}...", box_label(agent.display_name(), &project));
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
    oauth_volume: Option<&(String, String)>,
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
    labels.insert("agentbox.lifecycle".into(), "persistent".into());
    labels.insert("agentbox.box-name".into(), box_name.to_string());
    labels.insert("agentbox.agent-id".into(), agent.id().to_string());
    labels.insert(
        "agentbox.agent-display-name".into(),
        agent.display_name().to_string(),
    );
    labels.insert("agentbox.workdir".into(), agent.workdir().to_string());
    labels.insert("agentbox.launch-cmd".into(), launch_cmd_json);
    labels.insert("agentbox.folder".into(), host_folder_str.clone());
    if let Some(pn) = &cfg.project_name {
        labels.insert("agentbox.project-name".into(), pn.clone());
    }

    // State volume at /root; OAuth cache volume alongside it if needed.
    let mut volume_mounts = vec![(state_volume, "/root".to_string())];
    if let Some((vol, path)) = oauth_volume {
        // Don't double-mount if cache_path is under /root (already covered by state vol).
        if !path.starts_with("/root") {
            volume_mounts.push((vol.clone(), path.clone()));
        }
    }

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts,
        volume_mounts,
        env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels,
        cap_add: allowlist_caps(cfg),
        port_bindings: vec![],
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    let cleanup = CleanupGuard {
        docker,
        id: container_id.clone(),
        persistent: true,
    };

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    run_healthcheck(docker, &container_id, agent).await?;
    apply_egress_allowlist(docker, &container_id, cfg).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    let project = resolve_project_name(cfg.project_name.as_deref(), Path::new(&host_folder_str));
    eprintln!("Launching {}...", box_label(agent.display_name(), &project));
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir())
        .await?;

    drop(cleanup);

    if exit_code != 0 {
        eprintln!("Agent exited with code {exit_code}");
    }
    Ok(())
}

// ── Daemon launch ─────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_daemon(
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
    oauth_volume: Option<&(String, String)>,
    daemon_cfg: &crate::manifest::DaemonConfig,
) -> Result<(), EngineError> {
    let box_name = cfg.name.as_deref().unwrap(); // validated: daemon requires persistent + name
    let container_name = format!("agentbox-{box_name}");
    let state_volume = format!("agentbox-state-{box_name}");

    // If already running, just report status and return.
    if let Some(ContainerStatus::Running) = docker.container_status(&container_name).await {
        eprintln!("Daemon '{box_name}' is already running.");
        print_daemon_ports(daemon_cfg);
        return Ok(());
    }

    // Build port bindings from daemon manifest.
    let port_bindings: Vec<(u16, u16)> = daemon_cfg
        .ports
        .iter()
        .map(|p| (p.container_port, p.host_port))
        .collect();

    // Inject nested_sandbox env var if specified (e.g. HERMES_SANDBOX=local).
    let mut final_env_vars = env_vars;
    if let Some(sandbox_mode) = &daemon_cfg.nested_sandbox {
        final_env_vars.push(format!("HERMES_SANDBOX={sandbox_mode}"));
    }

    // Create state volume if needed.
    if !docker.volume_exists(&state_volume).await {
        docker.create_volume(&state_volume).await?;
    }

    let mut labels = HashMap::new();
    labels.insert("agentbox.managed".into(), "true".into());
    labels.insert("agentbox.lifecycle".into(), "persistent".into());
    labels.insert("agentbox.box-name".into(), box_name.to_string());
    labels.insert("agentbox.agent-id".into(), agent.id().to_string());
    labels.insert("agentbox.agent-display-name".into(), agent.display_name().to_string());
    labels.insert("agentbox.workdir".into(), agent.workdir().to_string());
    labels.insert("agentbox.launch-cmd".into(), launch_cmd_json);
    labels.insert("agentbox.folder".into(), host_folder_str);
    labels.insert("agentbox.daemon".into(), "true".into());
    if let Some(pn) = &cfg.project_name {
        labels.insert("agentbox.project-name".into(), pn.clone());
    }

    let mut volume_mounts = vec![(state_volume, "/root".to_string())];
    if let Some((vol, path)) = oauth_volume {
        if !path.starts_with("/root") {
            volume_mounts.push((vol.clone(), path.clone()));
        }
    }

    let spec = ContainerSpec {
        name: container_name.clone(),
        image: base_image,
        bind_mounts,
        volume_mounts,
        env_vars: final_env_vars,
        cpu_limit: cfg.resources.cpus,
        memory_limit,
        extra_hosts: host_gateway_hosts(),
        network_mode: "bridge".to_string(),
        workdir: agent.workdir().to_string(),
        labels,
        cap_add: allowlist_caps(cfg),
        port_bindings,
    };

    let container_id = docker.create_container(&spec).await?;
    docker.start_container(&container_id).await?;

    install_and_cache(docker, &container_id, agent, &cache_image, use_cache).await?;
    run_healthcheck(docker, &container_id, agent).await?;
    apply_egress_allowlist(docker, &container_id, cfg).await?;
    write_agent_config(docker, &container_id, agent, &cfg.provider, resolved_key).await?;

    // Run non-interactive setup if specified.
    if let Some(setup) = &daemon_cfg.setup {
        if setup.method == "exec" {
            if let Some(cmd) = &setup.command {
                eprint!("Running daemon setup ({})... ", cmd.join(" "));
                let result = docker.exec_command(&container_id, cmd, &[]).await?;
                if result.exit_code != 0 {
                    eprintln!("failed.");
                    docker.stop_container(&container_id).await.ok();
                    docker.remove_container(&container_id).await.ok();
                    return Err(EngineError::DaemonSetupFailed {
                        code: result.exit_code,
                        stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
                    });
                }
                eprintln!("done.");
            }
        }
    }

    // Launch the agent in the background (detached exec — stays running in container).
    eprint!("Starting {} daemon... ", agent.display_name());
    docker
        .exec_background(&container_id, &launch_cmd, agent.workdir())
        .await?;
    eprintln!("started.");

    eprintln!("\nDaemon '{box_name}' is running.");
    print_daemon_ports(daemon_cfg);
    eprintln!("Stop with: agentbox down {box_name}");

    Ok(())
}

fn print_daemon_ports(daemon_cfg: &crate::manifest::DaemonConfig) {
    for p in &daemon_cfg.ports {
        let flag = if p.optional { " (optional)" } else { "" };
        eprintln!("  localhost:{} → container:{}{}", p.host_port, p.container_port, flag);
    }
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
        if !cfg_bytes.is_empty() {
            docker.write_file(id, cfg_path, &cfg_bytes).await?;
        }
    }
    Ok(())
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Run the agent's healthcheck command after install. No-op if no healthcheck defined.
async fn run_healthcheck(
    docker: &DockerBackend,
    id: &ContainerId,
    agent: &dyn crate::agents::AgentDef,
) -> Result<(), EngineError> {
    let Some(cmd) = agent.healthcheck_command() else {
        return Ok(());
    };
    eprint!("Healthcheck ({})... ", cmd.join(" "));
    let result = docker.exec_command(id, &cmd, &[]).await?;
    if result.exit_code != 0 {
        eprintln!("failed.");
        return Err(EngineError::HealthcheckFailed {
            agent: agent.id().to_string(),
            code: result.exit_code,
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        });
    }
    eprintln!("ok.");
    Ok(())
}

/// Return `["NET_ADMIN"]` when the box uses the egress allowlist, else empty.
fn allowlist_caps(cfg: &BoxConfig) -> Vec<String> {
    if cfg.network == NetworkMode::Allowlist {
        vec!["NET_ADMIN".to_string()]
    } else {
        vec![]
    }
}

/// Apply egress iptables rules inside the container when `network: allowlist`.
/// This is a no-op for `network: open`.
async fn apply_egress_allowlist(
    docker: &DockerBackend,
    id: &ContainerId,
    cfg: &BoxConfig,
) -> Result<(), EngineError> {
    if cfg.network != NetworkMode::Allowlist {
        return Ok(());
    }

    eprintln!("Setting up egress allowlist...");

    // Install iptables in the container (Debian/Ubuntu base images).
    let install_result = docker
        .exec_command(
            id,
            &["sh".into(), "-c".into(),
              "apt-get install -y -qq iptables 2>/dev/null || true".into()],
            &[],
        )
        .await?;
    if install_result.exit_code != 0 {
        let stderr = String::from_utf8_lossy(&install_result.stderr).into_owned();
        return Err(EngineError::AllowlistSetup(
            format!("iptables install failed: {stderr}")
        ));
    }

    // Resolve allowed IPs from provider hostname.
    let allowed_ips = tokio::task::spawn_blocking({
        let cfg_clone = cfg.clone();
        move || resolve_provider_ips(&cfg_clone)
    })
    .await
    .unwrap_or_default();

    let script = build_allowlist_script(&allowed_ips);
    let result = docker
        .exec_command(id, &["sh".into(), "-c".into(), script], &[])
        .await?;
    if result.exit_code != 0 {
        let stderr = String::from_utf8_lossy(&result.stderr).into_owned();
        return Err(EngineError::AllowlistSetup(
            format!("iptables rules failed: {stderr}")
        ));
    }

    eprintln!("Egress allowlist active ({} provider IP(s)).", allowed_ips.len());
    Ok(())
}

/// Resolve the provider's API hostname(s) to IP strings.
fn resolve_provider_ips(cfg: &BoxConfig) -> Vec<String> {
    use std::net::ToSocketAddrs;

    let hostname = match cfg.provider.provider_type {
        ProviderType::Anthropic => "api.anthropic.com".to_string(),
        ProviderType::Openai => "api.openai.com".to_string(),
        ProviderType::OpenaiCompatible => {
            cfg.provider
                .base_url
                .as_deref()
                .and_then(extract_hostname)
                .unwrap_or_default()
                .to_string()
        }
    };

    if hostname.is_empty() {
        return vec![];
    }

    // Skip resolution for local/private addresses — the Docker network rule already covers them.
    if hostname == "host.docker.internal"
        || hostname == "localhost"
        || hostname == "127.0.0.1"
    {
        return vec![];
    }

    format!("{hostname}:443")
        .to_socket_addrs()
        .map(|iter| iter.map(|s| s.ip().to_string()).collect())
        .unwrap_or_default()
}

fn extract_hostname(url: &str) -> Option<&str> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme.split('/').next()?;
    Some(host.split(':').next().unwrap_or(host))
}

/// Build the shell script that applies DROP-by-default egress iptables rules.
fn build_allowlist_script(allowed_ips: &[String]) -> String {
    let mut rules: Vec<String> = vec![
        "iptables -F OUTPUT 2>/dev/null || true".into(),
        "iptables -A OUTPUT -o lo -j ACCEPT".into(),
        "iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT".into(),
        "iptables -A OUTPUT -p udp --dport 53 -j ACCEPT".into(),
        "iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT".into(),
        // Docker bridge networks (host gateway for local providers)
        "iptables -A OUTPUT -d 172.16.0.0/12 -j ACCEPT".into(),
        "iptables -A OUTPUT -d 192.168.0.0/16 -j ACCEPT".into(),
        "iptables -A OUTPUT -d 10.0.0.0/8 -j ACCEPT".into(),
    ];
    for ip in allowed_ips {
        rules.push(format!("iptables -A OUTPUT -d {ip} -j ACCEPT"));
    }
    rules.push("iptables -A OUTPUT -j DROP".into());
    rules.join("; ")
}

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

/// Slug from just the last path component (folder name), not the full path.
fn slug_from_basename(p: &Path) -> String {
    let name = p
        .file_name()
        .map(std::path::Path::new)
        .unwrap_or(p);
    slug_from_path(name)
}

/// Human-readable "Agent - Project" label for terminal output and window titles.
fn box_label(agent_display: &str, project_name: &str) -> String {
    format!("{agent_display} - {project_name}")
}

/// Resolve project_name: use explicit value if set, else folder basename.
fn resolve_project_name(cfg_project_name: Option<&str>, host_folder: &Path) -> String {
    if let Some(name) = cfg_project_name.filter(|s| !s.trim().is_empty()) {
        return name.to_owned();
    }
    host_folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| host_folder.to_string_lossy().into_owned())
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

/// Walk up from the running executable looking for a `manifests/` directory.
/// Handles cases where the binary lives inside a cargo target dir and the
/// manifests sit at the workspace root several levels above.
fn find_manifests_near_exe() -> Option<std::path::PathBuf> {
    let mut dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    while let Some(d) = dir {
        let candidate = d.join("manifests");
        if candidate.is_dir() {
            return Some(candidate);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}
