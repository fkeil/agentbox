use agentbox_core::ContainerStatus;
use clap::Args;

#[derive(Args)]
pub struct ListArgs {}

pub async fn run(_args: ListArgs) -> anyhow::Result<()> {
    let boxes = agentbox_core::list_boxes().await?;

    if boxes.is_empty() {
        println!("No persistent boxes found.");
        println!("Create one with: agentbox up --config box.yaml  (lifecycle: persistent)");
        return Ok(());
    }

    println!("{:<20} {:<20} {:<12} FOLDER", "NAME", "AGENT", "STATUS");
    println!("{}", "─".repeat(72));

    for b in &boxes {
        let status = match b.status {
            ContainerStatus::Running => "running",
            ContainerStatus::Stopped => "stopped",
        };
        let folder = b.folder.as_deref().unwrap_or("—");
        println!(
            "{:<20} {:<20} {:<12} {}",
            b.box_name, b.agent_display_name, status, folder
        );
    }

    Ok(())
}
