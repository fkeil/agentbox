use agentbox_core::manifest;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct AgentsArgs {
    /// Directory to search for agent manifests (default: ./manifests)
    #[arg(long)]
    pub manifests_dir: Option<PathBuf>,
}

pub async fn run(args: AgentsArgs) -> anyhow::Result<()> {
    let manifests_dir = args.manifests_dir.or_else(|| {
        let d = PathBuf::from("manifests");
        d.is_dir().then_some(d)
    });

    // Built-in agents (always available as fallback)
    let builtins: &[(&str, &str)] = &[("claude-code", "Claude Code"), ("opencode", "OpenCode")];

    // Manifest agents (override builtins when names match)
    let manifest_ids: std::collections::HashSet<String> = manifests_dir
        .as_deref()
        .map(manifest::list_manifests)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, _)| id)
        .collect();

    let all_manifests: Vec<(String, String)> = manifests_dir
        .as_deref()
        .map(manifest::list_manifests)
        .unwrap_or_default();

    println!("Available agents:");
    println!();

    // Manifest-defined agents first
    for (id, display_name) in &all_manifests {
        println!("  {id:<20} {display_name:<24} (manifest)");
    }

    // Builtins not superseded by a manifest
    for (id, display_name) in builtins {
        if !manifest_ids.contains(*id) {
            println!("  {id:<20} {display_name:<24} (built-in)");
        }
    }

    if let Some(dir) = &manifests_dir {
        println!();
        println!("Manifests directory: {}", dir.display());
    } else {
        println!();
        println!("No manifests/ directory found in the current working directory.");
    }

    Ok(())
}
