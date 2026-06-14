use async_trait::async_trait;
use bollard::container::{
    CreateContainerOptions, RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bollard::image::CreateImageOptions;
use bollard::models::{HostConfig, Mount, MountTypeEnum, Resources};
use futures_util::StreamExt;
use std::io::Write as _;
use std::path::Path;

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
    /// "KEY=VALUE" environment variables.
    pub env_vars: Vec<String>,
    pub cpu_limit: Option<f64>,
    pub memory_limit: Option<u64>,
    /// Extra `/etc/hosts` entries as "hostname:ip".
    pub extra_hosts: Vec<String>,
    pub network_mode: String,
    pub workdir: String,
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
}

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
}

pub struct DockerBackend {
    client: bollard::Docker,
}

impl DockerBackend {
    pub fn connect() -> Result<Self, ContainerError> {
        let client =
            bollard::Docker::connect_with_local_defaults().map_err(ContainerError::Connect)?;
        Ok(Self { client })
    }
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
        // Pull image if not already present.
        match self.client.inspect_image(&spec.image).await {
            Ok(_) => {}
            Err(_) => self.pull_image(&spec.image).await?,
        }

        let mounts: Vec<Mount> = spec
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

        let resources = Resources {
            nano_cpus: spec
                .cpu_limit
                .map(|c| (c * 1_000_000_000.0) as i64),
            memory: spec.memory_limit.map(|m| m as i64),
            ..Default::default()
        };

        let host_config = HostConfig {
            mounts: Some(mounts),
            network_mode: Some(spec.network_mode.clone()),
            extra_hosts: if spec.extra_hosts.is_empty() {
                None
            } else {
                Some(spec.extra_hosts.clone())
            },
            nano_cpus: resources.nano_cpus,
            memory: resources.memory,
            ..Default::default()
        };

        let env_strings: Vec<String> = spec.env_vars.clone();
        let env_refs: Vec<&str> = env_strings.iter().map(|s| s.as_str()).collect();

        let config = bollard::container::Config {
            image: Some(spec.image.as_str()),
            env: Some(env_refs),
            host_config: Some(host_config),
            // Keep the container running while we exec into it.
            cmd: Some(vec!["sleep", "infinity"]),
            working_dir: Some(spec.workdir.as_str()),
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
                        bollard::container::LogOutput::StdOut { message } => {
                            let _ = std::io::stdout().write_all(&message);
                            stdout.extend_from_slice(&message);
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            let _ = std::io::stderr().write_all(&message);
                            stderr.extend_from_slice(&message);
                        }
                        bollard::container::LogOutput::Console { message } => {
                            let _ = std::io::stdout().write_all(&message);
                            stdout.extend_from_slice(&message);
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

        // Ensure parent directory exists.
        self.exec_command(
            id,
            &["mkdir".into(), "-p".into(), parent_dir.clone()],
            &[],
        )
        .await?;

        // Build an in-memory tar archive containing one file.
        let mut tar_bytes = Vec::new();
        {
            let mut ar = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            ar.append_data(&mut header, &file_name, content)
                .map_err(|e| ContainerError::Tar(e.to_string()))?;
            ar.finish().map_err(|e| ContainerError::Tar(e.to_string()))?;
        }

        // Upload the tar to the parent directory inside the container.
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

        // Put host terminal into raw mode so keystrokes are forwarded correctly.
        // The guard restores normal mode on drop, including on panic.
        let _raw_guard = RawModeGuard::enable()?;

        // Capture host terminal dimensions before entering raw mode output loop.
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
                // Sync the container PTY size to the host terminal immediately.
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

                // Forward stdin from the host to the container in a background task.
                let stdin_task = tokio::spawn(async move {
                    let mut stdin = tokio::io::stdin();
                    let _ = tokio::io::copy(&mut stdin, &mut input).await;
                });

                // Forward container output to host stdout.
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
            .ok(); // best-effort
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
            .ok(); // best-effort
        Ok(())
    }
}

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
