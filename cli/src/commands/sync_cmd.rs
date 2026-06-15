use clap::{Args, Subcommand};

#[derive(Args)]
pub struct SyncArgs {
    #[command(subcommand)]
    pub command: SyncCommand,
}

#[derive(Subcommand)]
pub enum SyncCommand {
    /// Upload a box's state volume to a remote (rclone destination)
    Push(SyncTransferArgs),
    /// Restore a box's state volume from a remote (rclone source)
    Pull(SyncTransferArgs),
}

#[derive(Args)]
pub struct SyncTransferArgs {
    /// Box name (the persistent box whose state volume to sync)
    pub box_name: String,
    /// rclone remote path, e.g. `s3:my-bucket/agentbox` or `sftp:host:/path`
    pub remote: String,
}

pub async fn run(args: SyncArgs) -> anyhow::Result<()> {
    require_tool("rclone", "https://rclone.org/install/")?;
    require_tool("docker", "https://docs.docker.com/get-docker/")?;
    match args.command {
        SyncCommand::Push(a) => push(&a.box_name, &a.remote),
        SyncCommand::Pull(a) => pull(&a.box_name, &a.remote),
    }
}

fn require_tool(name: &str, install_url: &str) -> anyhow::Result<()> {
    let found = std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !found {
        anyhow::bail!(
            "`{name}` not found in PATH. Install it from {install_url} and run again."
        );
    }
    Ok(())
}

fn run_shell(cmd: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run shell: {e}"))?;
    if !status.success() {
        anyhow::bail!("command failed ({}): {cmd}", status);
    }
    Ok(())
}

fn push(box_name: &str, remote: &str) -> anyhow::Result<()> {
    let volume = format!("agentbox-state-{box_name}");
    let dest = format!("{remote}/{box_name}.tar.gz");
    eprintln!("Pushing state volume `{volume}` → `{dest}` …");

    let cmd = format!(
        "docker run --rm -v {volume}:/vol alpine tar czf - -C /vol . | rclone rcat {dest}"
    );
    run_shell(&cmd)?;
    eprintln!("Push complete.");
    Ok(())
}

fn pull(box_name: &str, remote: &str) -> anyhow::Result<()> {
    let volume = format!("agentbox-state-{box_name}");
    let src = format!("{remote}/{box_name}.tar.gz");
    eprintln!("Pulling `{src}` → state volume `{volume}` …");

    // Ensure volume exists (idempotent)
    let create_status = std::process::Command::new("docker")
        .args(["volume", "create", &volume])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run docker: {e}"))?;
    if !create_status.status.success() {
        anyhow::bail!("docker volume create failed");
    }

    let cmd = format!(
        "rclone cat {src} | docker run --rm -i -v {volume}:/vol alpine tar xzf - -C /vol"
    );
    run_shell(&cmd)?;
    eprintln!("Pull complete: volume `{volume}` restored.");
    Ok(())
}
