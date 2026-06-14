use anyhow::Context;
use clap::Args;

#[derive(Args)]
pub struct AttachArgs {
    /// Name of the persistent box to attach to
    pub box_name: String,
}

pub async fn run(args: AttachArgs) -> anyhow::Result<()> {
    agentbox_core::attach_box(&args.box_name)
        .await
        .with_context(|| format!("failed to attach to box `{}`", args.box_name))
}
