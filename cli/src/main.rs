mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agentbox", about = "Run AI agents in isolated containers")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start an agent box from a config file
    Up(commands::up::UpArgs),
    /// Stop and remove a named box
    Down(commands::down::DownArgs),
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Up(args) => commands::up::run(args).await,
        Command::Down(args) => commands::down::run(args).await,
    };
    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
