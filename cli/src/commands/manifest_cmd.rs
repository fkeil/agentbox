use agentbox_core::{add_manifest, list_user_manifests, remove_manifest};
use agentbox_core::manifest::list_manifests;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ManifestArgs {
    #[command(subcommand)]
    pub command: ManifestCommand,
}

#[derive(Subcommand)]
pub enum ManifestCommand {
    /// List available manifests (bundled + user-installed)
    List,
    /// Install a manifest from a URL or local file path
    Add {
        /// URL (https://…) or local file path to the manifest YAML
        source: String,
        /// Overwrite if a manifest with the same id already exists
        #[arg(long)]
        force: bool,
    },
    /// Remove a user-installed manifest by agent id
    Rm {
        /// Agent id (e.g. `my-agent`)
        id: String,
    },
}

pub async fn run(args: ManifestArgs) -> anyhow::Result<()> {
    match args.command {
        ManifestCommand::List => run_list(),
        ManifestCommand::Add { source, force } => run_add(&source, force).await,
        ManifestCommand::Rm { id } => run_rm(&id),
    }
}

fn run_list() -> anyhow::Result<()> {
    // Bundled manifests (near executable).
    let bundled_dir = agentbox_core::engine::find_manifests_dir_pub();
    let bundled: Vec<(String, String)> = bundled_dir
        .as_deref()
        .map(list_manifests)
        .unwrap_or_default();

    // User-installed manifests.
    let user = list_user_manifests();

    if bundled.is_empty() && user.is_empty() {
        println!("(no manifests found)");
        return Ok(());
    }

    println!("{:<20} {:<28} SOURCE", "ID", "DISPLAY NAME");
    println!("{}", "─".repeat(70));

    for (id, display_name) in &bundled {
        println!("{:<20} {:<28} bundled", id, display_name);
    }
    for m in &user {
        println!(
            "{:<20} {:<28} user (~/.config/agentbox/manifests/)",
            m.id, m.display_name
        );
    }
    Ok(())
}

async fn run_add(source: &str, force: bool) -> anyhow::Result<()> {
    let (id, dest) = add_manifest(source, force)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Manifest `{id}` installed → {}", dest.display());
    println!("Run: agentbox agents  (to verify it appears)");
    Ok(())
}

fn run_rm(id: &str) -> anyhow::Result<()> {
    remove_manifest(id).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("Manifest `{id}` removed from user store.");
    Ok(())
}
