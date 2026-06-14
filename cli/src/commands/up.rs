use anyhow::Context;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct UpArgs {
    /// Path to the box.yaml config file
    #[arg(long, short, default_value = "box.yaml")]
    pub config: PathBuf,
}

pub async fn run(args: UpArgs) -> anyhow::Result<()> {
    agentbox_core::run_box(&args.config)
        .await
        .with_context(|| format!("failed to run box from `{}`", args.config.display()))
}
