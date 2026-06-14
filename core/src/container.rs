use async_trait::async_trait;
use bollard::container::{
    CreateContainerOptions, DownloadFromContainerOptions, ListContainersOptions,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bollard::image::{CreateImageOptions, ListImagesOptions, RemoveImageOptions};
use bollard::models::{HostConfig, Mount, MountTypeEnum, PortBinding};
use bollard::volume::{CreateVolumeOptions, RemoveVolumeOptions};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContainerId(pub String);

#[derive(Debug)]
pub struct ExecResult {
    pub exit_code: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    /// (host_path, container_path) bind-mount pairs.
    pub bind_mounts: Vec<(String, String)>,
    /// (volume_name, container_path) named-volume mount pairs.
    pub volume_mounts: Vec<(String, String)>,
    /// "KEY=VALUE" environment variables.
    pub env_vars: Vec<String>,
    pub cpu_limit: Option<f64>,
    pub memory_limit: Option<u64>,
    /// Extra `/etc/hosts` entries as "hostname:ip".
    pub extra_hosts: Vec<String>,
    pub network_mode: String,
    pub workdir: String,
    /// Docker labels to attach to the container.
    pub labels: HashMap<String, String>,
    /// Linux capabilities to add (e.g. `["NET_ADMIN"]` for egress allowlist).
    pub cap_add: Vec<String>,
    /// Host port binding pairs (container_port, host_port) for daemon agents.
    pub port_bindings: Vec<(u16, u16)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContainerStatus {
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct BoxInfo {
    pub box_name: String,
    pub agent_id: String,
    pub agent_display_name: String,
    pub container_id: String,
    pub status: ContainerStatus,
    /// Host folder bind-mounted into the container, if readable from labels.
    pub folder: Option<String>,
    /// User-supplied project name (shown in window titles / box lists).
    pub project_name: Option<String>,
    /// "persistent" or "ephemeral". Ephemeral boxes in the list are orphaned leftovers.
    pub lifecycle: String,
    /// True if this is a daemon-mode agent (always-on service, not an interactive session).
    pub is_daemon: bool,
    /// Host ports bound by a daemon box (host_port:container_port pairs).
    pub bound_ports: Vec<(u16, u16)>,
}

/// A cached agent install image (`agentbox-cache-{agent_id}:latest`).
#[derive(Debug, Clone)]
pub struct CacheImage {
    pub agent_id: String,
    pub image_name: String,
    pub size_mb: f64,
    pub created_unix: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("failed to connect to Docker: {0}\nHint: is Docker running? Check `docker info`")]
    Connect(bollard::errors::Error),
    #[error("Docker API error: {0}")]
    Api(#[from] bollard::errors::Error),
    #[error(
        "agent install failed (exit={code}):\n{stderr}\nHint: check your network connection and try again"
    )]
    InstallFailed { code: i64, stderr: String },
    #[error("tar error building file upload: {0}")]
    Tar(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("box `{0}` not found")]
    BoxNotFound(String),
    #[error("microVM backend is not yet implemented; use backend: docker or backend: podman")]
    MicroVmNotSupported,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait ContainerBackend: Send + Sync {
    async fn pull_image(&self, image: &str) -> Result<(), ContainerError>;
    async fn create_container(&self, spec: &ContainerSpec) -> Result<ContainerId, ContainerError>;
    async fn start_container(&self, id: &ContainerId) -> Result<(), ContainerError>;
    async fn exec_command(
        &self,
        id: &ContainerId,
        cmd: &[String],
        env: &[String],
    ) -> Result<ExecResult, ContainerError>;
    async fn write_file(
        &self,
        id: &ContainerId,
        container_path: &str,
        content: &[u8],
    ) -> Result<(), ContainerError>;
    async fn attach_interactive(
        &self,
        id: &ContainerId,
        cmd: &[String],
        workdir: &str,
    ) -> Result<i64, ContainerError>;
    async fn stop_container(&self, id: &ContainerId) -> Result<(), ContainerError>;
    async fn remove_container(&self, id: &ContainerId) -> Result<(), ContainerError>;
    /// Returns true if an image with this exact name exists locally.
    async fn image_exists(&self, image: &str) -> bool;
    /// Commit the current container filesystem as a new local image.
    async fn commit_container(
        &self,
        id: &ContainerId,
        image_name: &str,
    ) -> Result<(), ContainerError>;
    /// Returns `Some(status)` if the named container exists, `None` if not.
    async fn container_status(&self, name: &str) -> Option<ContainerStatus>;
    /// Create a named Docker volume (no-op if it already exists).
    async fn create_volume(&self, name: &str) -> Result<(), ContainerError>;
    /// Returns true if a named volume exists.
    async fn volume_exists(&self, name: &str) -> bool;
    /// Remove a named volume (best-effort, ignores not-found).
    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError>;
    /// Return labels attached to a container.
    async fn get_container_labels(
        &self,
        id: &ContainerId,
    ) -> Result<HashMap<String, String>, ContainerError>;
    /// List all agentbox-managed containers (persistent + labeled ephemeral orphans).
    async fn list_boxes(&self) -> Result<Vec<BoxInfo>, ContainerError>;

    /// List cached agent install images (`agentbox-cache-*`).
    async fn list_cache_images(&self) -> Result<Vec<CacheImage>, ContainerError>;

    /// Remove a Docker image by its full name (e.g. `agentbox-cache-claude-code:latest`).
    async fn remove_image(&self, image: &str) -> Result<(), ContainerError>;

    /// Upload a local directory into the container at `container_path`.
    async fn copy_dir_to_container(
        &self,
        id: &ContainerId,
        local_dir: &std::path::Path,
        container_path: &str,
    ) -> Result<(), ContainerError>;

    /// Download a container directory as a raw tar archive.
    async fn download_dir(
        &self,
        id: &ContainerId,
        container_path: &str,
    ) -> Result<Vec<u8>, ContainerError>;
}

// ── Docker / Podman implementation ───────────────────────────────────────────

pub struct DockerBackend {
    client: bollard::Docker,
    /// Human-readable name of the backend in use (for error messages).
    pub backend_name: &'static str,
}

impl DockerBackend {
    /// Connect using the default Docker socket or `DOCKER_HOST`.
    pub fn connect() -> Result<Self, ContainerError> {
        Self::connect_with_backend(&crate::config::BackendChoice::Auto)
    }

    /// Connect using a specific backend choice from `box.yaml`.
    pub fn connect_with_backend(
        choice: &crate::config::BackendChoice,
    ) -> Result<Self, ContainerError> {
        use crate::config::BackendChoice;
        match choice {
            BackendChoice::Docker => {
                tracing::debug!("connecting to Docker socket");
                let client = bollard::Docker::connect_with_local_defaults()
                    .map_err(ContainerError::Connect)?;
                Ok(Self {
                    client,
                    backend_name: "docker",
                })
            }
            BackendChoice::Podman => {
                let socket = podman_socket_path();
                tracing::debug!("connecting to Podman socket: {socket}");
                let client =
                    bollard::Docker::connect_with_unix(&socket, 120, bollard::API_DEFAULT_VERSION)
                        .map_err(ContainerError::Connect)?;
                Ok(Self {
                    client,
                    backend_name: "podman",
                })
            }
            BackendChoice::Auto => {
                // Try DOCKER_HOST / default Docker socket first, then Podman.
                if std::env::var("DOCKER_HOST").is_ok() {
                    tracing::debug!("DOCKER_HOST set — using Docker backend");
                    return Self::connect_with_backend(&BackendChoice::Docker);
                }
                if std::path::Path::new("/var/run/docker.sock").exists() {
                    tracing::debug!("found /var/run/docker.sock — using Docker backend");
                    return Self::connect_with_backend(&BackendChoice::Docker);
                }
                let podman = podman_socket_path();
                if std::path::Path::new(&podman).exists() {
                    tracing::debug!("found Podman socket {podman} — using Podman backend");
                    return Self::connect_with_backend(&BackendChoice::Podman);
                }
                // Fall back to Docker defaults (will produce a clear error at first API call).
                tracing::debug!("no socket found; attempting Docker defaults");
                Self::connect_with_backend(&BackendChoice::Docker)
            }
            BackendChoice::Microvm => Err(ContainerError::MicroVmNotSupported),
        }
    }
}

/// Return the Podman user socket path following the XDG convention.
fn podman_socket_path() -> String {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return format!("{dir}/podman/podman.sock");
    }
    // Fallback: /run/user/<uid>/podman/podman.sock
    let uid = unsafe { libc::getuid() };
    format!("/run/user/{uid}/podman/podman.sock")
}

#[async_trait]
impl ContainerBackend for DockerBackend {
    async fn pull_image(&self, image: &str) -> Result<(), ContainerError> {
        eprintln!("Pulling image {image}...");
        let options = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };
        let mut stream = self.client.create_image(Some(options), None, None);
        while let Some(event) = stream.next().await {
            let info = event?;
            if let Some(status) = &info.status {
                eprint!("\r  {status}");
                if let Some(progress) = &info.progress {
                    eprint!(" {progress}");
                }
                let _ = std::io::stderr().flush();
            }
        }
        eprintln!();
        Ok(())
    }

    async fn create_container(&self, spec: &ContainerSpec) -> Result<ContainerId, ContainerError> {
        match self.client.inspect_image(&spec.image).await {
            Ok(_) => {}
            Err(_) => self.pull_image(&spec.image).await?,
        }

        let bind_mounts: Vec<Mount> = spec
            .bind_mounts
            .iter()
            .map(|(src, dst)| Mount {
                target: Some(dst.clone()),
                source: Some(src.clone()),
                typ: Some(MountTypeEnum::BIND),
                read_only: Some(false),
                ..Default::default()
            })
            .collect();

        let volume_mounts: Vec<Mount> = spec
            .volume_mounts
            .iter()
            .map(|(vol, dst)| Mount {
                target: Some(dst.clone()),
                source: Some(vol.clone()),
                typ: Some(MountTypeEnum::VOLUME),
                ..Default::default()
            })
            .collect();

        let all_mounts: Vec<Mount> = bind_mounts.into_iter().chain(volume_mounts).collect();

        let port_bindings: Option<HashMap<String, Option<Vec<PortBinding>>>> =
            if spec.port_bindings.is_empty() {
                None
            } else {
                Some(
                    spec.port_bindings
                        .iter()
                        .map(|(cp, hp)| {
                            let key = format!("{cp}/tcp");
                            let binding = PortBinding {
                                host_ip: Some("0.0.0.0".to_string()),
                                host_port: Some(hp.to_string()),
                            };
                            (key, Some(vec![binding]))
                        })
                        .collect(),
                )
            };

        let host_config = HostConfig {
            mounts: Some(all_mounts),
            network_mode: Some(spec.network_mode.clone()),
            extra_hosts: if spec.extra_hosts.is_empty() {
                None
            } else {
                Some(spec.extra_hosts.clone())
            },
            nano_cpus: spec.cpu_limit.map(|c| (c * 1_000_000_000.0) as i64),
            memory: spec.memory_limit.map(|m| m as i64),
            cap_add: if spec.cap_add.is_empty() {
                None
            } else {
                Some(spec.cap_add.clone())
            },
            port_bindings,
            ..Default::default()
        };

        let env_strings: Vec<String> = spec.env_vars.clone();
        let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

        let labels: HashMap<&str, &str> = spec
            .labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let config = bollard::container::Config {
            image: Some(spec.image.as_str()),
            env: Some(env_refs),
            host_config: Some(host_config),
            cmd: Some(vec!["sleep", "infinity"]),
            working_dir: Some(spec.workdir.as_str()),
            labels: if labels.is_empty() {
                None
            } else {
                Some(labels)
            },
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: spec.name.as_str(),
            platform: None,
        };

        let response = self.client.create_container(Some(options), config).await?;
        Ok(ContainerId(response.id))
    }

    async fn start_container(&self, id: &ContainerId) -> Result<(), ContainerError> {
        self.client
            .start_container(&id.0, None::<StartContainerOptions<String>>)
            .await?;
        Ok(())
    }

    async fn exec_command(
        &self,
        id: &ContainerId,
        cmd: &[String],
        env: &[String],
    ) -> Result<ExecResult, ContainerError> {
        let cmd_refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
        let env_refs: Vec<&str> = env.iter().map(|s| s.as_str()).collect();

        let exec_id = self
            .client
            .create_exec(
                &id.0,
                CreateExecOptions {
                    cmd: Some(cmd_refs),
                    env: if env_refs.is_empty() {
                        None
                    } else {
                        Some(env_refs)
                    },
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await?
            .id;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        match self
            .client
            .start_exec(&exec_id, None::<StartExecOptions>)
            .await?
        {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(chunk) = output.next().await {
                    match chunk? {
                        bollard::container::LogOutput::StdOut { message }
                        | bollard::container::LogOutput::Console { message } => {
                            stdout.extend_from_slice(&message);
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            stderr.extend_from_slice(&message);
                        }
                        _ => {}
                    }
                }
            }
            StartExecResults::Detached => {}
        }

        let inspect = self.client.inspect_exec(&exec_id).await?;
        let exit_code = inspect.exit_code.unwrap_or(0);

        Ok(ExecResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn write_file(
        &self,
        id: &ContainerId,
        container_path: &str,
        content: &[u8],
    ) -> Result<(), ContainerError> {
        let path = Path::new(container_path);
        let file_name = path
            .file_name()
            .ok_or_else(|| ContainerError::Tar(format!("invalid path: {container_path}")))?
            .to_string_lossy()
            .into_owned();
        let parent_dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());

        self.exec_command(id, &["mkdir".into(), "-p".into(), parent_dir.clone()], &[])
            .await?;

        let mut tar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, &file_name, content)
                .map_err(|e| ContainerError::Tar(e.to_string()))?;
            ar.finish()
                .map_err(|e| ContainerError::Tar(e.to_string()))?;
        }

        let upload_dir = format!("{}/", parent_dir.trim_end_matches('/'));
        self.client
            .upload_to_container(
                &id.0,
                Some(bollard::container::UploadToContainerOptions {
                    path: upload_dir,
                    no_overwrite_dir_non_dir: "".into(),
                }),
                tar_bytes.into(),
            )
            .await?;

        Ok(())
    }

    async fn attach_interactive(
        &self,
        id: &ContainerId,
        cmd: &[String],
        workdir: &str,
    ) -> Result<i64, ContainerError> {
        let cmd_refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();

        let exec_id = self
            .client
            .create_exec(
                &id.0,
                CreateExecOptions {
                    cmd: Some(cmd_refs),
                    attach_stdin: Some(true),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    working_dir: Some(workdir),
                    ..Default::default()
                },
            )
            .await?
            .id;

        let _raw_guard = RawModeGuard::enable()?;
        let term_size = crossterm::terminal::size().ok();

        match self
            .client
            .start_exec(
                &exec_id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    ..Default::default()
                }),
            )
            .await?
        {
            StartExecResults::Attached {
                mut output,
                mut input,
            } => {
                if let Some((cols, rows)) = term_size {
                    let _ = self
                        .client
                        .resize_exec(
                            &exec_id,
                            ResizeExecOptions {
                                height: rows,
                                width: cols,
                            },
                        )
                        .await;
                }

                let stdin_task = tokio::spawn(async move {
                    let mut stdin = tokio::io::stdin();
                    let _ = tokio::io::copy(&mut stdin, &mut input).await;
                });

                while let Some(chunk) = output.next().await {
                    match chunk? {
                        bollard::container::LogOutput::Console { message }
                        | bollard::container::LogOutput::StdOut { message }
                        | bollard::container::LogOutput::StdErr { message } => {
                            let _ = std::io::stdout().write_all(&message);
                            let _ = std::io::stdout().flush();
                        }
                        _ => {}
                    }
                }

                stdin_task.abort();
            }
            StartExecResults::Detached => {}
        }

        let inspect = self.client.inspect_exec(&exec_id).await?;
        Ok(inspect.exit_code.unwrap_or(0))
    }

    async fn stop_container(&self, id: &ContainerId) -> Result<(), ContainerError> {
        self.client
            .stop_container(&id.0, Some(StopContainerOptions { t: 5 }))
            .await
            .ok();
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<(), ContainerError> {
        self.client
            .remove_container(
                &id.0,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .ok();
        Ok(())
    }

    async fn image_exists(&self, image: &str) -> bool {
        self.client.inspect_image(image).await.is_ok()
    }

    async fn commit_container(
        &self,
        id: &ContainerId,
        image_name: &str,
    ) -> Result<(), ContainerError> {
        let (repo, tag) = image_name.split_once(':').unwrap_or((image_name, "latest"));
        self.client
            .commit_container(
                bollard::image::CommitContainerOptions {
                    container: id.0.as_str(),
                    repo,
                    tag,
                    ..Default::default()
                },
                bollard::container::Config::<String>::default(),
            )
            .await?;
        Ok(())
    }

    async fn container_status(&self, name: &str) -> Option<ContainerStatus> {
        let info = self.client.inspect_container(name, None).await.ok()?;
        let state_str = info.state?.status?.to_string();
        if state_str == "running" {
            Some(ContainerStatus::Running)
        } else {
            Some(ContainerStatus::Stopped)
        }
    }

    async fn create_volume(&self, name: &str) -> Result<(), ContainerError> {
        self.client
            .create_volume(CreateVolumeOptions {
                name: name.to_string(),
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    async fn volume_exists(&self, name: &str) -> bool {
        self.client.inspect_volume(name).await.is_ok()
    }

    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError> {
        self.client
            .remove_volume(name, None::<RemoveVolumeOptions>)
            .await
            .ok();
        Ok(())
    }

    async fn get_container_labels(
        &self,
        id: &ContainerId,
    ) -> Result<HashMap<String, String>, ContainerError> {
        let info = self
            .client
            .inspect_container(&id.0, None)
            .await
            .map_err(|_| ContainerError::BoxNotFound(id.0.clone()))?;
        Ok(info
            .config
            .and_then(|c| c.labels)
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect())
    }

    async fn list_boxes(&self) -> Result<Vec<BoxInfo>, ContainerError> {
        let mut filters = HashMap::new();
        filters.insert("label", vec!["agentbox.managed=true"]);

        let containers = self
            .client
            .list_containers(Some(ListContainersOptions {
                all: true,
                filters,
                ..Default::default()
            }))
            .await?;

        let mut boxes: Vec<BoxInfo> = containers
            .into_iter()
            .filter_map(|c| {
                let labels = c.labels.unwrap_or_default();
                let box_name = labels.get("agentbox.box-name")?.to_string();
                let agent_id = labels.get("agentbox.agent-id").cloned().unwrap_or_default();
                let agent_display_name = labels
                    .get("agentbox.agent-display-name")
                    .cloned()
                    .unwrap_or_else(|| agent_id.clone());
                let folder = labels.get("agentbox.folder").cloned();
                let project_name = labels.get("agentbox.project-name").cloned();
                let status = c
                    .state
                    .as_deref()
                    .map(|s| {
                        if s == "running" {
                            ContainerStatus::Running
                        } else {
                            ContainerStatus::Stopped
                        }
                    })
                    .unwrap_or(ContainerStatus::Stopped);

                let lifecycle = labels
                    .get("agentbox.lifecycle")
                    .cloned()
                    .unwrap_or_else(|| "persistent".to_string());

                let is_daemon = labels
                    .get("agentbox.daemon")
                    .map(|v| v == "true")
                    .unwrap_or(false);

                // Extract bound ports from the Docker port list (daemon boxes only).
                let bound_ports: Vec<(u16, u16)> = c
                    .ports
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|p| Some((p.public_port?, p.private_port)))
                    .collect();

                Some(BoxInfo {
                    box_name,
                    agent_id,
                    agent_display_name,
                    container_id: c.id.unwrap_or_default(),
                    status,
                    folder,
                    project_name,
                    lifecycle,
                    is_daemon,
                    bound_ports,
                })
            })
            .collect();

        boxes.sort_by(|a, b| a.box_name.cmp(&b.box_name));
        Ok(boxes)
    }

    async fn list_cache_images(&self) -> Result<Vec<CacheImage>, ContainerError> {
        let images = self
            .client
            .list_images(Some(ListImagesOptions::<String> {
                all: false,
                filters: HashMap::new(),
                digests: false,
            }))
            .await?;

        let mut result: Vec<CacheImage> = images
            .into_iter()
            .filter_map(|img| {
                let repo_tag = img
                    .repo_tags
                    .into_iter()
                    .find(|t| t.starts_with("agentbox-cache-"))?;
                let agent_id = repo_tag
                    .strip_prefix("agentbox-cache-")?
                    .trim_end_matches(":latest")
                    .to_string();
                Some(CacheImage {
                    agent_id,
                    image_name: repo_tag,
                    size_mb: (img.size as f64) / (1024.0 * 1024.0),
                    created_unix: img.created,
                })
            })
            .collect();

        result.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        Ok(result)
    }

    async fn remove_image(&self, image: &str) -> Result<(), ContainerError> {
        self.client
            .remove_image(
                image,
                Some(RemoveImageOptions {
                    force: true,
                    noprune: false,
                }),
                None,
            )
            .await?;
        Ok(())
    }

    async fn copy_dir_to_container(
        &self,
        id: &ContainerId,
        local_dir: &std::path::Path,
        container_path: &str,
    ) -> Result<(), ContainerError> {
        let mut tar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut tar_bytes);
            ar.append_dir_all(".", local_dir)
                .map_err(|e| ContainerError::Tar(e.to_string()))?;
            ar.finish()
                .map_err(|e| ContainerError::Tar(e.to_string()))?;
        }

        self.exec_command(
            id,
            &["mkdir".into(), "-p".into(), container_path.to_string()],
            &[],
        )
        .await?;

        let upload_path = format!("{}/", container_path.trim_end_matches('/'));
        self.client
            .upload_to_container(
                &id.0,
                Some(bollard::container::UploadToContainerOptions {
                    path: upload_path,
                    no_overwrite_dir_non_dir: "".into(),
                }),
                tar_bytes.into(),
            )
            .await?;

        Ok(())
    }

    async fn download_dir(
        &self,
        id: &ContainerId,
        container_path: &str,
    ) -> Result<Vec<u8>, ContainerError> {
        let mut stream = self.client.download_from_container(
            &id.0,
            Some(DownloadFromContainerOptions {
                path: container_path.to_string(),
            }),
        );

        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk?);
        }

        Ok(bytes)
    }
}

impl DockerBackend {
    /// Launch a command inside the container in the background (detached exec).
    /// Returns immediately; the command keeps running until the container stops.
    pub async fn exec_background(
        &self,
        id: &ContainerId,
        cmd: &[String],
        workdir: &str,
    ) -> Result<(), ContainerError> {
        let cmd_refs: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();

        let exec_id = self
            .client
            .create_exec(
                &id.0,
                CreateExecOptions {
                    cmd: Some(cmd_refs),
                    attach_stdin: Some(false),
                    attach_stdout: Some(false),
                    attach_stderr: Some(false),
                    working_dir: Some(workdir),
                    ..Default::default()
                },
            )
            .await?
            .id;

        self.client
            .start_exec(
                &exec_id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await?;

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// RAII guard that puts the terminal into raw mode and restores it on drop.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self, ContainerError> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
