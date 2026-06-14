use clap::{Args, Subcommand};

#[derive(Args)]
pub struct ImagesArgs {
    #[command(subcommand)]
    pub command: Option<ImagesCommand>,
}

#[derive(Subcommand)]
pub enum ImagesCommand {
    /// Remove a specific agent's cache image
    Rm {
        /// Agent ID (e.g. claude-code, opencode, pi)
        agent_id: String,
    },
    /// Remove all agentbox cache images
    Prune,
}

pub async fn run(args: ImagesArgs) -> anyhow::Result<()> {
    match args.command {
        None => list_images().await,
        Some(ImagesCommand::Rm { agent_id }) => {
            agentbox_core::remove_cache_image(&agent_id).await?;
            println!("Removed cache image for '{agent_id}'.");
            Ok(())
        }
        Some(ImagesCommand::Prune) => {
            let images = agentbox_core::list_cache_images().await?;
            if images.is_empty() {
                println!("No cache images found.");
                return Ok(());
            }
            for img in &images {
                agentbox_core::remove_cache_image(&img.agent_id).await?;
                println!("Removed {}", img.image_name);
            }
            println!("Pruned {} cache image(s).", images.len());
            Ok(())
        }
    }
}

async fn list_images() -> anyhow::Result<()> {
    let images = agentbox_core::list_cache_images().await?;

    if images.is_empty() {
        println!("No agentbox cache images found.");
        println!("Cache images are created on first launch of each agent.");
        return Ok(());
    }

    println!("{:<20} {:<40} SIZE", "AGENT", "IMAGE");
    println!("{}", "─".repeat(72));

    for img in &images {
        println!(
            "{:<20} {:<40} {:.1} MB",
            img.agent_id, img.image_name, img.size_mb
        );
    }

    Ok(())
}
