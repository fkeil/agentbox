use agentbox_core::{
    config::Lifecycle, find_manifests_dir_pub, list_profiles, load_profile, remove_profile,
    run_box_config, save_profile, Profile, ProfileError,
};
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
    /// Save a box.yaml file as a named profile
    Save(ProfileSaveArgs),
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
    #[arg(long, value_parser = parse_lifecycle)]
    pub lifecycle: Option<Lifecycle>,
}

#[derive(Args)]
pub struct ProfileSaveArgs {
    /// Profile name to save as
    pub name: String,
    /// Path to a box.yaml to read agent + provider settings from
    #[arg(long)]
    pub from: PathBuf,
    /// Overwrite an existing profile with the same name
    #[arg(long)]
    pub force: bool,
}

fn parse_lifecycle(s: &str) -> Result<Lifecycle, String> {
    match s {
        "ephemeral" => Ok(Lifecycle::Ephemeral),
        "persistent" => Ok(Lifecycle::Persistent),
        other => Err(format!("unknown lifecycle `{other}`; use ephemeral or persistent")),
    }
}

pub async fn run(args: ProfileArgs) -> anyhow::Result<()> {
    match args.command {
        ProfileCommand::List => run_list(),
        ProfileCommand::Show { name } => run_show(&name),
        ProfileCommand::Rm { name } => run_rm(&name),
        ProfileCommand::Run(args) => run_run(args).await,
        ProfileCommand::Save(args) => run_save(args),
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
    let profile = load_profile(&args.name).map_err(|e| anyhow::anyhow!("{e}"))?;
    let cfg = profile.into_box_config(args.folder, args.box_name, args.lifecycle);
    agentbox_core::config::validate_config(&cfg)?;
    let manifests_dir = find_manifests_dir_pub();
    run_box_config(cfg, manifests_dir.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))
}

fn run_save(args: ProfileSaveArgs) -> anyhow::Result<()> {
    let cfg = agentbox_core::config::parse_config(&args.from)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let profile = Profile {
        name: args.name.clone(),
        agent: cfg.agent.0,
        provider: cfg.provider,
        network: cfg.network,
        resources: cfg.resources,
        extra_env: cfg.extra_env,
        backend: cfg.backend,
        lifecycle: cfg.lifecycle,
    };
    let path = save_profile(&profile, args.force).map_err(|e| match e {
        ProfileError::AlreadyExists(n) => {
            anyhow::anyhow!("profile `{n}` already exists; use --force to overwrite")
        }
        other => anyhow::anyhow!("{other}"),
    })?;
    println!("Profile `{}` saved → {}", args.name, path.display());
    Ok(())
}
