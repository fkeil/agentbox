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

    println!(
        "{:<22} {:<20} {:<10} {:<12} FOLDER / PORTS",
        "NAME", "AGENT", "STATUS", "MODE"
    );
    println!("{}", "─".repeat(90));

    for b in &boxes {
        let status = match b.status {
            ContainerStatus::Running => "running",
            ContainerStatus::Stopped => "stopped",
        };

        let detail = if b.is_daemon && !b.bound_ports.is_empty() {
            b.bound_ports
                .iter()
                .map(|(h, c)| format!(":{h}→{c}"))
                .collect::<Vec<_>>()
                .join("  ")
        } else {
            b.folder.as_deref().unwrap_or("—").to_string()
        };

        let mode = if b.is_daemon {
            "daemon"
        } else {
            b.lifecycle.as_str()
        };

        println!(
            "{:<22} {:<20} {:<10} {:<12} {}",
            b.box_name, b.agent_display_name, status, mode, detail
        );
    }

    let orphaned: Vec<_> = boxes
        .iter()
        .filter(|b| b.lifecycle == "ephemeral" && !b.is_daemon)
        .collect();
    if !orphaned.is_empty() {
        println!(
            "\n{} orphaned ephemeral container(s). Remove with:",
            orphaned.len()
        );
        for b in &orphaned {
            println!("  agentbox kill {}", b.box_name);
        }
    }

    Ok(())
}
