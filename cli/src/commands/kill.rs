use clap::Args;

#[derive(Args)]
pub struct KillArgs {
    /// Box name to force-remove (same name shown by `agentbox list`)
    pub name: String,
}

pub async fn run(args: KillArgs) -> anyhow::Result<()> {
    agentbox_core::kill_box(&args.name).await?;
    println!("Removed.");
    Ok(())
}
