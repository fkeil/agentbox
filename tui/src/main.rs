mod wizard;

use agentbox_core::engine;

#[tokio::main]
async fn main() {
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

    let outcome = match result {
        wizard::WizardResult::Cancelled => Ok(()),
        wizard::WizardResult::Launch { config, manifests_dir } => {
            engine::run_box_config(*config, manifests_dir.as_deref()).await
        }
        wizard::WizardResult::Attach { box_name } => engine::attach_box(&box_name).await,
    };

    if let Err(e) = outcome {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
