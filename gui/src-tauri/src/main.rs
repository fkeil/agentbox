#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── DTO types (passed over the Tauri IPC boundary) ───────────────────────────

#[derive(Serialize)]
struct BoxInfoDto {
    box_name: String,
    agent_id: String,
    agent_display_name: String,
    project_name: Option<String>,
    status: String,
    folder: Option<String>,
    lifecycle: String,
}

impl From<agentbox_core::BoxInfo> for BoxInfoDto {
    fn from(b: agentbox_core::BoxInfo) -> Self {
        Self {
            status: match b.status {
                agentbox_core::ContainerStatus::Running => "running".into(),
                agentbox_core::ContainerStatus::Stopped => "stopped".into(),
            },
            box_name: b.box_name,
            agent_id: b.agent_id,
            agent_display_name: b.agent_display_name,
            project_name: b.project_name,
            folder: b.folder,
            lifecycle: b.lifecycle,
        }
    }
}

#[derive(Serialize)]
struct CacheImageDto {
    agent_id: String,
    image_name: String,
    size_mb: f64,
    created_unix: i64,
}

impl From<agentbox_core::CacheImage> for CacheImageDto {
    fn from(img: agentbox_core::CacheImage) -> Self {
        Self {
            agent_id: img.agent_id,
            image_name: img.image_name,
            size_mb: (img.size_mb * 10.0).round() / 10.0,
            created_unix: img.created_unix,
        }
    }
}

#[derive(Serialize)]
struct AgentEntryDto {
    id: String,
    display_name: String,
    source: String,
    oauth_supported: bool,
}

#[derive(Serialize)]
struct FileDiffDto {
    path: String,
    kind: String,
    patch: String,
}

impl From<agentbox_core::FileDiff> for FileDiffDto {
    fn from(d: agentbox_core::FileDiff) -> Self {
        Self {
            kind: match d.kind {
                agentbox_core::DiffKind::Added => "added".into(),
                agentbox_core::DiffKind::Modified => "modified".into(),
                agentbox_core::DiffKind::Deleted => "deleted".into(),
            },
            path: d.path,
            patch: d.patch,
        }
    }
}

#[derive(Deserialize)]
struct ProviderInput {
    name: String,
    #[serde(rename = "type")]
    provider_type: String,
    model: String,
    base_url: Option<String>,
    auth: String,
}

#[derive(Deserialize)]
struct BoxConfigInput {
    agent: String,
    name: Option<String>,
    project_name: Option<String>,
    folder: String,
    lifecycle: String,
    sync: String,
    provider: ProviderInput,
    /// Pi-specific: full models.json content as a JSON string (optional)
    pi_models_json: Option<String>,
}

#[derive(Serialize)]
struct LaunchInfo {
    config_path: String,
    diff_path: String,
    container_hint: String,
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
async fn get_boxes() -> Result<Vec<BoxInfoDto>, String> {
    agentbox_core::list_boxes()
        .await
        .map_err(|e| e.to_string())
        .map(|v| v.into_iter().map(Into::into).collect())
}

#[tauri::command]
async fn get_agents() -> Vec<AgentEntryDto> {
    use agentbox_core::manifest;
    let manifests_dir = find_manifests_dir();

    let mut entries: Vec<AgentEntryDto> = manifests_dir
        .as_ref()
        .map(|d| {
            manifest::list_manifests_meta(d)
                .into_iter()
                .map(|m| AgentEntryDto {
                    oauth_supported: m.oauth_supported,
                    id: m.id,
                    display_name: m.display_name,
                    source: "manifest".into(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Hardcoded fallbacks not already covered by a manifest
    let manifest_ids: std::collections::HashSet<String> =
        entries.iter().map(|e| e.id.clone()).collect();
    for (id, name, oauth) in [
        ("claude-code", "Claude Code", true),
        ("opencode", "OpenCode", false),
    ] {
        if !manifest_ids.contains(id) {
            entries.push(AgentEntryDto {
                id: id.into(),
                display_name: name.into(),
                source: "builtin".into(),
                oauth_supported: oauth,
            });
        }
    }

    entries
}

/// Write a temporary box.yaml and return the paths the GUI needs.
#[tauri::command]
async fn prepare_launch(config: BoxConfigInput) -> Result<LaunchInfo, String> {
    let provider_type = match config.provider.provider_type.as_str() {
        "openai" => agentbox_core::config::ProviderType::Openai,
        "openai-compatible" => agentbox_core::config::ProviderType::OpenaiCompatible,
        _ => agentbox_core::config::ProviderType::Anthropic,
    };

    let lifecycle = match config.lifecycle.as_str() {
        "persistent" => agentbox_core::config::Lifecycle::Persistent,
        _ => agentbox_core::config::Lifecycle::Ephemeral,
    };

    let sync = match config.sync.as_str() {
        "snapshot" => agentbox_core::config::SyncMode::Snapshot,
        _ => agentbox_core::config::SyncMode::Mount,
    };

    let folder_path = PathBuf::from(&config.folder);

    let box_cfg = agentbox_core::config::BoxConfig {
        agent: agentbox_core::config::AgentId(config.agent),
        name: config.name,
        project_name: config.project_name.filter(|s| !s.trim().is_empty()),
        folder: agentbox_core::config::FolderConfig {
            path: folder_path.clone(),
            sync,
        },
        lifecycle,
        provider: agentbox_core::config::ProviderConfig {
            name: config.provider.name,
            provider_type,
            model: config.provider.model,
            base_url: config.provider.base_url,
            auth: config.provider.auth,
            raw: config.pi_models_json
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::Value::Null),
        },
        network: agentbox_core::config::NetworkMode::Open,
        resources: agentbox_core::config::ResourceConfig::default(),
        extra_env: std::collections::HashMap::new(),
        backend: agentbox_core::config::BackendChoice::Auto,
    };

    let yaml = serde_yaml::to_string(&box_cfg).map_err(|e| e.to_string())?;

    let config_path = std::env::temp_dir().join("agentbox-gui-launch.yaml");
    std::fs::write(&config_path, yaml).map_err(|e| e.to_string())?;

    let diff_path = agentbox_core::diff_path_for(&folder_path);
    // Remove stale diff from a previous run so the GUI knows when a new one arrives.
    std::fs::remove_file(&diff_path).ok();

    let config_path_str = config_path.to_string_lossy().into_owned();
    let diff_path_str = diff_path.to_string_lossy().into_owned();

    Ok(LaunchInfo {
        config_path: config_path_str,
        diff_path: diff_path_str,
        container_hint: String::new(),
    })
}

/// Open a system terminal and run `agentbox attach <box-name>`.
#[tauri::command]
async fn attach_box_terminal(box_name: String, title: Option<String>) -> Result<(), String> {
    let bin = find_agentbox_bin();
    let title_esc = title
        .as_deref()
        .map(|t| format!("printf \"\\033]0;{}\\007\"; ", t.replace('"', "\\\"")))
        .unwrap_or_default();
    let cmd = format!("{title_esc}'{bin}' attach '{box_name}'");
    launch_terminal(&cmd).map_err(|e| e.to_string())
}

/// Open a system terminal and run `agentbox up --config <path>`.
#[tauri::command]
async fn open_in_terminal(config_path: String, title: Option<String>) -> Result<(), String> {
    let bin = find_agentbox_bin();
    let title_esc = title
        .as_deref()
        .map(|t| format!("printf \"\\033]0;{}\\007\"; ", t.replace('"', "\\\"")))
        .unwrap_or_default();
    let cmd = format!("{title_esc}'{bin}' up --config '{config_path}'");
    launch_terminal(&cmd).map_err(|e| e.to_string())
}

/// Find the bundled `manifests/` directory by walking up from the exe.
fn find_manifests_dir() -> Option<std::path::PathBuf> {
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

/// Find the `agentbox` CLI binary. Walks up from the running executable's
/// directory looking for `target/{debug,release}/agentbox` in ancestor dirs,
/// then falls back to ~/.cargo/bin, then bare "agentbox" (PATH).
fn find_agentbox_bin() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        while let Some(d) = dir {
            for profile in &["debug", "release"] {
                let candidate = d.join("target").join(profile).join("agentbox");
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin/agentbox");
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }
    "agentbox".to_string()
}

/// Return the snapshot diff for `host_folder` if the diff file exists.
#[tauri::command]
async fn get_snapshot_diff(host_folder: String) -> Option<Vec<FileDiffDto>> {
    let folder = PathBuf::from(host_folder);
    agentbox_core::load_diff(&folder)
        .map(|diffs| diffs.into_iter().map(Into::into).collect())
}

/// Apply approved changes from the snapshot diff back to the host directory.
#[tauri::command]
async fn apply_snapshot_changes(
    host_folder: String,
    approved_paths: Vec<String>,
) -> Result<(), String> {
    agentbox_core::apply_snapshot_diff(
        std::path::Path::new(&host_folder),
        &approved_paths,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn stop_box(box_name: String) -> Result<(), String> {
    agentbox_core::stop_box(&box_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn remove_box(box_name: String) -> Result<(), String> {
    agentbox_core::remove_box(&box_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn kill_box(box_name: String) -> Result<(), String> {
    agentbox_core::kill_box(&box_name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn list_cache_images() -> Result<Vec<CacheImageDto>, String> {
    agentbox_core::list_cache_images()
        .await
        .map_err(|e| e.to_string())
        .map(|v| v.into_iter().map(Into::into).collect())
}

#[tauri::command]
async fn remove_cache_image(agent_id: String) -> Result<(), String> {
    agentbox_core::remove_cache_image(&agent_id)
        .await
        .map_err(|e| e.to_string())
}

// ── Terminal launcher ─────────────────────────────────────────────────────────

fn launch_terminal(cmd: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(format!(
                r#"tell application "Terminal" to do script "{cmd}""#
            ))
            .spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let arg_cmd = format!("sh -c '{cmd}; echo; read -p \"Press Enter to close...\" _'");
        // Try terminals in order of prevalence
        let candidates: &[(&str, &[&str])] = &[
            ("x-terminal-emulator", &["--"]),
            ("gnome-terminal", &["--"]),
            ("xterm", &["-e"]),
            ("konsole", &["-e"]),
            ("xfce4-terminal", &["-e"]),
        ];
        for (term, flag) in candidates {
            if std::process::Command::new(term)
                .args(*flag)
                .arg("sh")
                .arg("-c")
                .arg(&arg_cmd)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no terminal emulator found (tried x-terminal-emulator, gnome-terminal, xterm, konsole)",
        ));
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "cmd", "/k", cmd])
            .spawn()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                if let Some(icon) = app.default_window_icon() {
                    window.set_icon(icon.clone()).ok();
                }
            }
            Ok(())
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            get_boxes,
            get_agents,
            prepare_launch,
            open_in_terminal,
            attach_box_terminal,
            get_snapshot_diff,
            apply_snapshot_changes,
            stop_box,
            remove_box,
            kill_box,
            list_cache_images,
            remove_cache_image,
        ])
        .run(tauri::generate_context!())
        .expect("error while running agentbox GUI");
}
