mod wizard;

use agentbox_core::engine;

#[tokio::main]
async fn main() {
    // The wizard runs synchronously (crossterm event loop); we spawn it on a
    // blocking thread so the async runtime isn't blocked.
    let result = match tokio::task::spawn_blocking(wizard::run).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            eprintln!("TUI error: {e}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Internal error: {e}");
            std::process::exit(1);
        }
    };

    match result {
        wizard::WizardResult::Cancelled => {}
        wizard::WizardResult::Launch { config, manifests_dir } => {
            if let Err(e) = engine::run_box_config(*config, manifests_dir.as_deref()).await {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
        }
    }
}
