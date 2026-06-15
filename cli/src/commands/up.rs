use anyhow::Context;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct UpArgs {
    /// Path(s) to the box.yaml config file (repeat for multi-box)
    #[arg(long, short, default_value = "box.yaml")]
    pub config: Vec<PathBuf>,
    /// Validate and print what would happen without starting any containers
    #[arg(long)]
    pub dry_run: bool,
    /// Override Docker host (e.g. ssh://user@host); also settable in box.yaml via `remote:`
    #[arg(long)]
    pub remote: Option<String>,
}

pub async fn run(args: UpArgs) -> anyhow::Result<()> {
    if let Some(remote) = &args.remote {
        std::env::set_var("DOCKER_HOST", remote);
    }

    if args.dry_run {
        for path in &args.config {
            agentbox_core::dry_run_box(path)
                .await
                .with_context(|| format!("dry-run failed for `{}`", path.display()))?;
        }
        return Ok(());
    }

    if args.config.len() == 1 {
        agentbox_core::run_box(&args.config[0])
            .await
            .with_context(|| {
                format!("failed to run box from `{}`", args.config[0].display())
            })
    } else {
        // Multi-box: run sequentially; interactive sessions cannot share the terminal.
        let total = args.config.len();
        let mut failed = 0usize;
        for (i, path) in args.config.iter().enumerate() {
            eprintln!(
                "\n[{}/{}] Starting box from `{}`…",
                i + 1,
                total,
                path.display()
            );
            if let Err(e) = agentbox_core::run_box(path).await {
                eprintln!("[{}/{}] Error: {e:#}", i + 1, total);
                failed += 1;
            } else {
                eprintln!("[{}/{}] Done.", i + 1, total);
            }
        }
        if failed > 0 {
            anyhow::bail!("{failed}/{total} box(es) failed");
        }
        Ok(())
    }
}
