use agentbox_core::ContainerStatus;
use clap::Args;

#[derive(Args)]
pub struct ListArgs {}

pub async fn run(_args: ListArgs) -> anyhow::Result<()> {
    let boxes = agentbox_core::list_boxes().await?;

    if boxes.is_empty() {
        println!("No boxes found.");
        println!("Run:  agentbox up --config box.yaml");
        return Ok(());
    }

    println!("{:<22} {:<20} {:<10} {:<12} FOLDER", "NAME", "AGENT", "STATUS", "LIFECYCLE");
    println!("{}", "─".repeat(86));

    for b in &boxes {
        let status = match b.status {
            ContainerStatus::Running => "running",
            ContainerStatus::Stopped => "stopped",
        };
        let folder = b.folder.as_deref().unwrap_or("—");
        println!(
            "{:<22} {:<20} {:<10} {:<12} {}",
            b.box_name, b.agent_display_name, status, b.lifecycle, folder
        );
    }

    let orphaned: Vec<_> = boxes.iter().filter(|b| b.lifecycle == "ephemeral").collect();
    if !orphaned.is_empty() {
        println!("\n{} orphaned ephemeral container(s). Remove with:", orphaned.len());
        for b in &orphaned {
            println!("  agentbox kill {}", b.box_name);
        }
    }

    Ok(())
}
