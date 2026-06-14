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
    /// Attach to (resume) an existing persistent box
    Attach(commands::attach::AttachArgs),
    /// Stop and remove a named persistent box (container + state volume)
    Down(commands::down::DownArgs),
    /// List all boxes (persistent + orphaned ephemeral containers)
    List(commands::list::ListArgs),
    /// Force-remove a container without deleting its state volume
    Kill(commands::kill::KillArgs),
    /// Manage cached agent install images
    Images(commands::images::ImagesArgs),
    /// List available agents (manifests + built-ins)
    Agents(commands::agents::AgentsArgs),
}

fn print_banner() {
    // ANSI colors: cyan = \x1b[36m, dim = \x1b[2m, bold = \x1b[1m, reset = \x1b[0m
    const C: &str = "\x1b[36m";   // cyan — box/robot structure
    const D: &str = "\x1b[2;37m"; // dim white — decorative / dim parts
    const B: &str = "\x1b[1;36m"; // bold cyan — wordmark
    const W: &str = "\x1b[37m";   // white — tagline
    const E: &str = "\x1b[1;97m"; // bright bold — eyes
    const R: &str = "\x1b[0m";    // reset

    println!();
    println!("       {C}·:·:·{R}");
    println!("        {D}|||{R}");
    println!("   {C}╔═════════╗{R}   {B}a g e n t b o x{R}");
    println!("   {C}║ {E}◉{R}     {E}◉{R} {C}║{R}   {D}────────────────────────{R}");
    println!("   {C}║  {D}─────{R}  {C}║{R}   {W}run AI agents in isolated containers{R}");
    println!("   {C}╚═════╤═══╝{R}");
    println!(" {C}╔═══════╧═════╗{R}");
    println!(" {C}║             ║{R}");
    println!(" {C}╚═════════════╝{R}");
    println!();
}

#[tokio::main]
async fn main() {
    // Show banner when invoked with no subcommand (help screen) or --help/-h
    let args: Vec<String> = std::env::args().collect();
    let show_banner = args.len() == 1
        || args.iter().any(|a| a == "--help" || a == "-h");
    if show_banner {
        print_banner();
    }

    let cli = Cli::parse();
    let result = match cli.command {
        Command::Up(args) => commands::up::run(args).await,
        Command::Attach(args) => commands::attach::run(args).await,
        Command::Down(args) => commands::down::run(args).await,
        Command::List(args) => commands::list::run(args).await,
        Command::Kill(args) => commands::kill::run(args).await,
        Command::Images(args) => commands::images::run(args).await,
        Command::Agents(args) => commands::agents::run(args).await,
    };
    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
