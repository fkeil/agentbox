mod wizard;

use agentbox_core::engine;

#[tokio::main]
async fn main() {
    // Structured logging — file-based to avoid corrupting the terminal UI.
    // RUST_LOG controls verbosity; output goes to /tmp/agentbox-tui.log.
    if let Ok(log_file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/agentbox-tui.log")
    {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_writer(std::sync::Mutex::new(log_file))
            .without_time()
            .compact()
            .init();
    }

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
