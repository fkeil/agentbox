use anyhow::Context;
use clap::Args;

#[derive(Args)]
pub struct DownArgs {
    /// Name of the box to stop (required for persistent boxes in Phase 3)
    pub name: Option<String>,
}

pub async fn run(args: DownArgs) -> anyhow::Result<()> {
    let name = args.name.as_deref().unwrap_or("default");
    agentbox_core::down_box(name)
        .await
        .with_context(|| format!("failed to stop box `{name}`"))
}
