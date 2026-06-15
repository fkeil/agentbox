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
    #[error(
        "unknown agent `{0}`. Built-ins: claude-code, opencode. Or add a manifests/{0}.yaml file."
    )]
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
    BadFolderPath {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("task join error: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error(
        "agent `{0}` does not support in-container OAuth; use `auth: ${{env:API_KEY_VAR}}` instead"
    )]
    OAuthNotSupported(String),
    #[error("healthcheck failed for agent `{agent}` (exit {code}):\n{stderr}")]
    HealthcheckFailed {
        agent: String,
        code: i64,
        stderr: String,
    },
    #[error("egress allowlist setup failed: {0}")]
    AllowlistSetup(String),
    #[error("daemon agent `{0}` requires `lifecycle: persistent`; set `lifecycle: persistent` and `name: <box-name>` in box.yaml")]
    DaemonRequiresPersistent(String),
    #[error("daemon setup failed (exit={code}):\n{stderr}")]
    DaemonSetupFailed { code: i64, stderr: String },
    #[error("before-hook `{cmd}` failed (exit {code})")]
    HookFailed { cmd: String, code: i32 },
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

/// Validate and print what would happen without touching Docker.
pub async fn dry_run_box(config_path: &Path) -> Result<(), EngineError> {
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

    let agent = crate::manifest_store::find_manifest_with_user_store(
        manifests_dir.as_deref(),
        &cfg.agent.0,
    )
    .map(|m| {
        Box::new(crate::agents::manifest_agent::ManifestAgentDef::new(m))
            as Box<dyn crate::agents::AgentDef>
    })
    .or_else(|| agents::find_agent(&cfg.agent.0, manifests_dir.as_deref()))
    .ok_or_else(|| EngineError::UnknownAgent(cfg.agent.0.clone()))?;

    provider::check_provider_compat(
        agent.id(),
        &cfg.provider.provider_type,
        agent.supported_providers(),
    )?;

    let auth_display = {
        let r = &cfg.provider.auth;
        if r == "none" || r == "oauth" {
            r.clone()
        } else if let Some(name) = r.strip_prefix("${env:").and_then(|s| s.strip_suffix('}')) {
            let status = if std::env::var(name).is_ok() { "set" } else { "NOT SET" };
            format!("{r} ({status})")
        } else {
            r.clone()
        }
    };

    let host_folder = cfg
        .folder
        .path
        .canonicalize()
        .map_err(|e| EngineError::BadFolderPath {
            path: cfg.folder.path.clone(),
            source: e,
        })?;

    println!("─── dry-run — no containers will be created ───────────");
    println!("  agent        : {} ({})", cfg.agent.0, agent.display_name());
    println!("  folder       : {}", host_folder.display());
    println!("  sync         : {:?}", cfg.folder.sync);
    println!("  lifecycle    : {:?}", cfg.lifecycle);
    if let Some(name) = &cfg.name {
        println!("  box name     : {name}");
    }
    println!(
        "  provider     : {} ({:?})",
        cfg.provider.name, cfg.provider.provider_type
    );
    println!("  model        : {}", cfg.provider.model);
    if let Some(url) = &cfg.provider.base_url {
        println!("  base_url     : {url}");
    }
    println!("  auth         : {auth_display}");
    println!("  network      : {:?}", cfg.network);
    println!("  backend      : {:?}", cfg.backend);
    if !cfg.extra_mounts.is_empty() {
        println!("  extra mounts :");
        for m in &cfg.extra_mounts {
            let mode = if m.readonly { ":ro" } else { ":rw" };
            println!("    {} → {}{mode}", m.path.display(), m.container_path);
        }
    }
    if !cfg.hooks.before.is_empty() || !cfg.hooks.after.is_empty() {
        println!("  hooks        :");
        for h in &cfg.hooks.before {
            println!("    before: {h}");
        }
        for h in &cfg.hooks.after {
            println!("    after : {h}");
        }
    }
    println!("  image        : {}", agent.base_image());
    println!("───────────────────────────────────────────────────────");

    Ok(())
}

/// Run a box from a pre-parsed config. Called by the TUI after the wizard
/// collects settings.
pub async fn run_box_config(
    cfg: BoxConfig,
    manifests_dir: Option<&Path>,
) -> Result<(), EngineError> {
    // Find agent: user manifest store → bundled manifests dir → built-ins.
    let agent = crate::manifest_store::find_manifest_with_user_store(manifests_dir, &cfg.agent.0)
        .map(|m| {
            Box::new(crate::agents::manifest_agent::ManifestAgentDef::new(m))
                as Box<dyn crate::agents::AgentDef>
        })
        .or_else(|| agents::find_agent(&cfg.agent.0, manifests_dir))
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

    if let Some(remote) = &cfg.remote {
        std::env::set_var("DOCKER_HOST", remote);
    }
    let docker = DockerBackend::connect_with_backend(&cfg.backend)?;
    tracing::info!(backend = docker.backend_name, agent = %cfg.agent.0, "starting box");

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

    // Run before-hooks on the host (abort launch on failure).
    run_before_hooks(&cfg.hooks.before)?;

    // Set up guard that runs after-hooks and optional OS notification on session end.
    let _end_guard = SessionEndGuard {
        hooks: cfg.hooks.after.clone(),
        notifications: cfg.notifications,
    };

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
    let base_image = if use_cache {
        cache_image.clone()
    } else {
        agent.base_image().to_string()
    };

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

    let mut bind_mounts = vec![(
        host_folder.to_string_lossy().into_owned(),
        agent.workdir().to_string(),
    )];
    for mount in &cfg.extra_mounts {
        let container_path = if mount.readonly {
            format!("{}:ro", mount.container_path)
        } else {
            mount.container_path.clone()
        };
        bind_mounts.push((mount.path.to_string_lossy().into_owned(), container_path));
    }

    // Daemon agents bypass the normal lifecycle branches.
    if let Some(daemon_cfg) = agent.daemon_config() {
        if cfg.lifecycle != Lifecycle::Persistent {
            return Err(EngineError::DaemonRequiresPersistent(
                agent.id().to_string(),
            ));
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

    // Daemon boxes are not interactive — just report their running status.
    if labels
        .get("agentbox.daemon")
        .map(|v| v == "true")
        .unwrap_or(false)
    {
        let status = docker.container_status(&container_name).await;
        match status {
            Some(ContainerStatus::Running) => {
                eprintln!("Daemon '{box_name}' is running.");
                // Show bound ports from live container info.
                if let Ok(boxes) = docker.list_boxes().await {
                    if let Some(b) = boxes.iter().find(|b| b.box_name == box_name) {
                        for (h, c) in &b.bound_ports {
                            eprintln!("  localhost:{h} → container:{c}");
                        }
                    }
                }
            }
            Some(ContainerStatus::Stopped) => {
                eprintln!("Daemon '{box_name}' is stopped.");
                eprintln!("Start with: agentbox up --config <box.yaml>");
            }
            None => {
                return Err(EngineError::Container(ContainerError::BoxNotFound(
                    box_name.to_string(),
                )));
            }
        }
        return Ok(());
    }

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

    let agent_display = labels
        .get("agentbox.agent-display-name")
        .map(|s| s.as_str())
        .unwrap_or(box_name);
    let project = labels
        .get("agentbox.project-name")
        .cloned()
        .unwrap_or_else(|| box_name.to_string());
    eprintln!("Attaching to box '{box_name}'...");
    let title = box_label(agent_display, &project);
    set_terminal_title(&title);
    let exit_code = docker.attach_interactive(&id, &launch_cmd, workdir, Some(&title)).await?;

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

    let diffs = crate::sync::load_diff(host_folder).ok_or_else(|| {
        EngineError::Container(ContainerError::BoxNotFound(format!(
            "no snapshot diff found for {}",
            host_folder.display()
        )))
    })?;
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
    labels.insert(
        "agentbox.agent-display-name".into(),
        agent.display_name().to_string(),
    );
    labels.insert(
        "agentbox.folder".into(),
        host_folder.to_string_lossy().into_owned(),
    );
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
    let title = box_label(agent.display_name(), &project);
    eprintln!("Launching {}...", title);
    set_terminal_title(&title);
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir(), Some(&title))
        .await?;

    print_egress_log(docker, &container_id, cfg).await;
    print_cost_estimate(docker, &container_id, agent).await;

    eprintln!("Computing diff...");
    let diffs =
        crate::sync::compute_snapshot_diff(docker, &container_id, agent.workdir(), &host_folder)
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
    labels.insert(
        "agentbox.agent-display-name".into(),
        agent.display_name().to_string(),
    );
    labels.insert(
        "agentbox.folder".into(),
        host_folder.to_string_lossy().into_owned(),
    );
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
    let title = box_label(agent.display_name(), &project);
    eprintln!("Launching {}...", title);
    set_terminal_title(&title);
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir(), Some(&title))
        .await?;

    print_git_summary(docker, &container_id).await;
    print_egress_log(docker, &container_id, cfg).await;
    print_cost_estimate(docker, &container_id, agent).await;

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
        let project = resolve_project_name(cfg.project_name.as_deref(), Path::new(&host_folder_str));
        let title = box_label(agent.display_name(), &project);
        eprintln!("Reconnecting to box '{box_name}'...");
        set_terminal_title(&title);
        let exit_code = docker
            .attach_interactive(&id, &launch_cmd, agent.workdir(), Some(&title))
            .await?;
        print_git_summary(docker, &id).await;
        print_egress_log(docker, &id, cfg).await;
        print_cost_estimate(docker, &id, agent).await;
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
    let title = box_label(agent.display_name(), &project);
    eprintln!("Launching {}...", title);
    set_terminal_title(&title);
    let exit_code = docker
        .attach_interactive(&container_id, &launch_cmd, agent.workdir(), Some(&title))
        .await?;

    print_git_summary(docker, &container_id).await;
    print_egress_log(docker, &container_id, cfg).await;
    print_cost_estimate(docker, &container_id, agent).await;

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

    // If the container already exists, handle without re-creating.
    match docker.container_status(&container_name).await {
        Some(ContainerStatus::Running) => {
            eprintln!("Daemon '{box_name}' is already running.");
            print_daemon_ports(daemon_cfg);
            return Ok(());
        }
        Some(ContainerStatus::Stopped) => {
            let id = ContainerId(container_name.clone());
            docker.start_container(&id).await?;
            eprintln!("Daemon '{box_name}' restarted.");
            print_daemon_ports(daemon_cfg);
            eprintln!("Stop with: agentbox down {box_name}");
            return Ok(());
        }
        None => {}
    }

    // Build port bindings from daemon manifest.
    let port_bindings: Vec<(u16, u16)> = daemon_cfg
        .ports
        .iter()
        .map(|p| (p.container_port, p.host_port))
        .collect();

    // nested_sandbox env var is now injected via manifest.env (agent-specific).
    // Keep as alias for backward-compat with hermes.yaml stub.
    let mut final_env_vars = env_vars;
    if let Some(sandbox_mode) = &daemon_cfg.nested_sandbox {
        // Only inject if manifest.env doesn't already set a sandbox var.
        let already_set = final_env_vars.iter().any(|v| v.contains("SANDBOX"));
        if !already_set {
            final_env_vars.push(format!("HERMES_SANDBOX={sandbox_mode}"));
        }
    }

    // Inject agentbox-internal meta vars so daemon config_file templates can use them.
    let provider_type_str = match cfg.provider.provider_type {
        ProviderType::Anthropic => "anthropic",
        ProviderType::Openai => "openai",
        ProviderType::OpenaiCompatible => "openai-compatible",
    };
    final_env_vars.push(format!("PROVIDER_TYPE={provider_type_str}"));
    final_env_vars.push(format!("MODEL={}", cfg.provider.model));
    final_env_vars.push(format!("API_KEY={resolved_key}"));
    if let Some(base_url) = &cfg.provider.base_url {
        final_env_vars.push(format!("BASE_URL={base_url}"));
    } else {
        final_env_vars.push("BASE_URL=".to_string());
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
    labels.insert(
        "agentbox.agent-display-name".into(),
        agent.display_name().to_string(),
    );
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
        env_vars: final_env_vars.clone(),
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
        match setup.method.as_str() {
            "exec" => {
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
            "config_file" => {
                // Render the template and write it to the specified path inside the container.
                if let (Some(path), Some(template)) = (&setup.config_path, &setup.config_template) {
                    eprint!("Writing daemon config to {path}... ");
                    // Render {{var}} placeholders the same way agent configs are rendered.
                    let rendered = render_daemon_template(template, &final_env_vars);
                    // Ensure the parent directory exists.
                    let parent = std::path::Path::new(path)
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("/tmp");
                    let mkdir_cmd = vec!["mkdir".into(), "-p".into(), parent.to_string()];
                    docker.exec_command(&container_id, &mkdir_cmd, &[]).await?;
                    docker
                        .write_file(&container_id, path, rendered.as_bytes())
                        .await?;
                    eprintln!("done.");
                }
            }
            // "env" — all configuration is via env vars already injected; nothing extra needed.
            _ => {}
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
        eprintln!(
            "  localhost:{} → container:{}{}",
            p.host_port, p.container_port, flag
        );
    }
}

/// Substitute `{{ENV_VAR}}` placeholders in a daemon config template by
/// looking up the value in the injected env var list (`KEY=VALUE` pairs).
fn render_daemon_template(template: &str, env_vars: &[String]) -> String {
    let env_map: std::collections::HashMap<&str, &str> = env_vars
        .iter()
        .filter_map(|kv| kv.split_once('='))
        .collect();

    let mut out = template.to_owned();
    for (k, v) in &env_map {
        out = out.replace(&format!("{{{{{k}}}}}"), v);
    }
    out
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
        tracing::info!(agent = agent.id(), "using cached install image");
        eprintln!("Using cached {} image.", agent.id());
    } else {
        tracing::info!(agent = agent.id(), "installing agent");
        eprint!("Installing {}... ", agent.id());
        let result = docker
            .exec_command(id, &agent.install_command(), &[])
            .await?;
        if result.exit_code != 0 {
            tracing::error!(
                agent = agent.id(),
                exit_code = result.exit_code,
                "install failed"
            );
            eprintln!("failed.");
            return Err(EngineError::Container(ContainerError::InstallFailed {
                code: result.exit_code,
                stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
            }));
        }
        eprintln!("done.");
        tracing::debug!(agent = agent.id(), cache_image, "committing cache image");
        eprint!("Caching image... ");
        match docker.commit_container(id, cache_image).await {
            Ok(()) => eprintln!("done."),
            Err(e) => {
                tracing::warn!(error = %e, "could not cache agent install image");
                eprintln!("warning: could not cache image: {e}");
            }
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
            if provider.auth != "none" {
                Some(resolved_key)
            } else {
                None
            },
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
            &[
                "sh".into(),
                "-c".into(),
                "apt-get install -y -qq iptables 2>/dev/null || true".into(),
            ],
            &[],
        )
        .await?;
    if install_result.exit_code != 0 {
        let stderr = String::from_utf8_lossy(&install_result.stderr).into_owned();
        return Err(EngineError::AllowlistSetup(format!(
            "iptables install failed: {stderr}"
        )));
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
        return Err(EngineError::AllowlistSetup(format!(
            "iptables rules failed: {stderr}"
        )));
    }

    eprintln!(
        "Egress allowlist active ({} provider IP(s)).",
        allowed_ips.len()
    );
    Ok(())
}

/// Resolve the provider's API hostname(s) to IP strings.
fn resolve_provider_ips(cfg: &BoxConfig) -> Vec<String> {
    use std::net::ToSocketAddrs;

    let hostname = match cfg.provider.provider_type {
        ProviderType::Anthropic => "api.anthropic.com".to_string(),
        ProviderType::Openai => "api.openai.com".to_string(),
        ProviderType::OpenaiCompatible => cfg
            .provider
            .base_url
            .as_deref()
            .and_then(extract_hostname)
            .unwrap_or_default()
            .to_string(),
    };

    if hostname.is_empty() {
        return vec![];
    }

    // Skip resolution for local/private addresses — the Docker network rule already covers them.
    if hostname == "host.docker.internal" || hostname == "localhost" || hostname == "127.0.0.1" {
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
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_lowercase()
        .chars()
        .take(40)
        .collect()
}

/// Slug from just the last path component (folder name), not the full path.
fn slug_from_basename(p: &Path) -> String {
    let name = p.file_name().map(std::path::Path::new).unwrap_or(p);
    slug_from_path(name)
}

/// Human-readable "Agent - Project" label for terminal output and window titles.
fn box_label(agent_display: &str, project_name: &str) -> String {
    format!("{agent_display} - {project_name}")
}

/// Emit an OSC 0 terminal title escape sequence directly to stdout.
/// Called right before attach_interactive so the title persists into the session.
fn set_terminal_title(title: &str) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(format!("\x1b]0;{title}\x07").as_bytes());
    let _ = std::io::stdout().flush();
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
                eprint!("\nPress ENTER to close window...");
                let _ = std::io::stdin().read_line(&mut String::new());
            });
        });
    }
}

/// Run before-hooks on the host. Returns error on first failure.
fn run_before_hooks(hooks: &[String]) -> Result<(), EngineError> {
    for cmd in hooks {
        eprint!("[before-hook] {cmd}... ");
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .status();
        match status {
            Ok(s) if s.success() => eprintln!("ok."),
            Ok(s) => {
                let code = s.code().unwrap_or(-1);
                eprintln!("failed (exit {code}).");
                return Err(EngineError::HookFailed {
                    cmd: cmd.clone(),
                    code,
                });
            }
            Err(e) => {
                eprintln!("error: {e}");
                return Err(EngineError::HookFailed {
                    cmd: cmd.clone(),
                    code: -1,
                });
            }
        }
    }
    Ok(())
}

/// Drop guard: runs after-hooks and optional OS notification when the session ends.
/// This fires even when the session returns an error.
struct SessionEndGuard {
    hooks: Vec<String>,
    notifications: bool,
}

impl Drop for SessionEndGuard {
    fn drop(&mut self) {
        for cmd in &self.hooks {
            eprint!("[after-hook] {cmd}... ");
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .status()
            {
                Ok(s) if s.success() => eprintln!("ok."),
                Ok(s) => eprintln!("failed (exit {}).", s.code().unwrap_or(-1)),
                Err(e) => eprintln!("error: {e}"),
            }
        }
        if self.notifications {
            crate::notify::send_notification("Agentbox", "Agent session ended.");
        }
    }
}

/// Get a one-shot CPU/memory snapshot for a named container.
pub async fn get_container_stats(
    container_name: &str,
) -> Result<crate::container::ContainerStats, EngineError> {
    let docker = DockerBackend::connect()?;
    let id = ContainerId(container_name.to_string());
    Ok(docker.container_stats_once(&id).await?)
}

// ── Post-session summary helpers ──────────────────────────────────────────────

/// Print git diff --stat after the session (best-effort; silently skips on error).
async fn print_git_summary(docker: &DockerBackend, id: &ContainerId) {
    let cmd = vec![
        "sh".into(),
        "-c".into(),
        "test -d .git && git diff --stat HEAD 2>/dev/null; git status --short 2>/dev/null | head -20"
            .into(),
    ];
    let Ok(result) = docker.exec_command(id, &cmd, &[]).await else {
        return;
    };
    let output = String::from_utf8_lossy(&result.stdout);
    let trimmed = output.trim();
    if !trimmed.is_empty() {
        eprintln!("\n─── git summary ───────────────────────────────────────");
        eprintln!("{trimmed}");
        eprintln!("───────────────────────────────────────────────────────");
    }
}

/// Print dropped-packet counts from iptables when network: allowlist (best-effort).
async fn print_egress_log(docker: &DockerBackend, id: &ContainerId, cfg: &BoxConfig) {
    if cfg.network != NetworkMode::Allowlist {
        return;
    }
    let cmd = vec!["iptables".into(), "-nL".into(), "OUTPUT".into(), "-v".into()];
    let Ok(result) = docker.exec_command(id, &cmd, &[]).await else {
        return;
    };
    let output = String::from_utf8_lossy(&result.stdout);
    let (mut dropped_pkts, mut blocked_dests) = (0u64, std::collections::HashSet::new());
    for line in output.lines() {
        if line.contains("DROP") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if let Some(pkts) = parts.first().and_then(|s| s.parse::<u64>().ok()) {
                dropped_pkts += pkts;
            }
            // destination IP is typically the 9th field in verbose iptables output
            if parts.len() >= 9 {
                let dest = parts[8];
                if dest != "0.0.0.0/0" {
                    blocked_dests.insert(dest.to_string());
                }
            }
        }
    }
    if dropped_pkts > 0 || !blocked_dests.is_empty() {
        eprintln!(
            "Egress: {} packet(s) to {} destination(s) blocked.",
            dropped_pkts,
            blocked_dests.len()
        );
    }
}

/// Estimate and print session cost from in-container token logs (best-effort).
async fn print_cost_estimate(
    docker: &DockerBackend,
    id: &ContainerId,
    agent: &dyn crate::agents::AgentDef,
) {
    let Some(cost_cfg) = agent.cost_config() else {
        return;
    };
    if cost_cfg.post_session_cmd.is_empty() {
        return;
    }
    let Ok(result) = docker
        .exec_command(id, &cost_cfg.post_session_cmd, &[])
        .await
    else {
        return;
    };
    let output = String::from_utf8_lossy(&result.stdout);
    let input_tokens = parse_token_field(&output, "input_tokens");
    let output_tokens = parse_token_field(&output, "output_tokens");
    if input_tokens == 0 && output_tokens == 0 {
        return;
    }
    let cost_usd = (input_tokens as f64 / 1_000_000.0) * cost_cfg.input_per_1m
        + (output_tokens as f64 / 1_000_000.0) * cost_cfg.output_per_1m;
    eprintln!(
        "Estimated cost: ${:.4} ({} input / {} output tokens)",
        cost_usd,
        format_tokens(input_tokens),
        format_tokens(output_tokens),
    );
}

/// Find the last occurrence of `"field":NNN` in `text` and parse the number.
fn parse_token_field(text: &str, field: &str) -> u64 {
    let needle = format!("\"{}\":", field);
    text.rfind(&needle)
        .and_then(|pos| {
            let rest = &text[pos + needle.len()..];
            rest.split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Public wrapper so the CLI manifest command can find the bundled manifests dir.
pub fn find_manifests_dir_pub() -> Option<std::path::PathBuf> {
    find_manifests_near_exe()
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
