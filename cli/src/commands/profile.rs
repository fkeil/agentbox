use agentbox_core::{list_profiles, load_profile, remove_profile, run_box_config, ProfileError};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    /// List all saved profiles
    List,
    /// Show a profile's contents
    Show { name: String },
    /// Delete a saved profile
    Rm { name: String },
    /// Run a box using a named profile
    Run(ProfileRunArgs),
}

#[derive(Args)]
pub struct ProfileRunArgs {
    /// Profile name
    pub name: String,
    /// Host folder to mount into the container
    #[arg(long, short)]
    pub folder: PathBuf,
    /// Box name (required if the profile lifecycle is persistent)
    #[arg(long)]
    pub box_name: Option<String>,
    /// Override lifecycle (ephemeral or persistent)
    #[arg(long)]
    pub lifecycle: Option<String>,
}

pub async fn run(args: ProfileArgs) -> anyhow::Result<()> {
    match args.command {
        ProfileCommand::List => run_list(),
        ProfileCommand::Show { name } => run_show(&name),
        ProfileCommand::Rm { name } => run_rm(&name),
        ProfileCommand::Run(args) => run_run(args).await,
    }
}

fn run_list() -> anyhow::Result<()> {
    let profiles = list_profiles().unwrap_or_default();
    if profiles.is_empty() {
        println!("No profiles saved.");
        println!("Create one by copying a box.yaml provider + agent block into:");
        println!("  ~/.config/agentbox/profiles/<name>.yaml");
        return Ok(());
    }
    println!("{:<24} {:<20} PROVIDER", "NAME", "AGENT");
    println!("{}", "─".repeat(70));
    for p in &profiles {
        println!(
            "{:<24} {:<20} {} / {}",
            p.name, p.agent, p.provider.name, p.provider.model
        );
    }
    Ok(())
}

fn run_show(name: &str) -> anyhow::Result<()> {
    let p = load_profile(name).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Profile: {}", p.name);
    println!("  agent:    {}", p.agent);
    println!("  provider: {} ({:?})", p.provider.name, p.provider.provider_type);
    println!("  model:    {}", p.provider.model);
    println!("  network:  {:?}", p.network);
    println!("  backend:  {:?}", p.backend);
    if !p.extra_env.is_empty() {
        println!("  extra_env:");
        for (k, v) in &p.extra_env {
            println!("    {k}={v}");
        }
    }
    Ok(())
}

fn run_rm(name: &str) -> anyhow::Result<()> {
    remove_profile(name).map_err(|e| match e {
        ProfileError::NotFound(n) => anyhow::anyhow!("profile `{n}` not found"),
        other => anyhow::anyhow!("{other}"),
    })?;
    println!("Profile `{name}` removed.");
    Ok(())
}

async fn run_run(args: ProfileRunArgs) -> anyhow::Result<()> {
    let profile = load_profile(&args.name)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let lifecycle_override = match args.lifecycle.as_deref() {
        Some("persistent") => Some(agentbox_core::config::Lifecycle::Persistent),
        Some("ephemeral") | None => None,
        Some(other) => anyhow::bail!("unknown lifecycle `{other}`; use ephemeral or persistent"),
    };

    let cfg = profile.into_box_config(args.folder, args.box_name, lifecycle_override);
    agentbox_core::config::validate_config(&cfg)?;
    run_box_config(cfg, None).await.map_err(|e| anyhow::anyhow!("{e:#}"))
}
