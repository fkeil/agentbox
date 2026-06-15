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
    /// Manage saved provider + agent profiles (presets)
    Profile(commands::profile::ProfileArgs),
    /// Manage user-installed agent manifests
    Manifest(commands::manifest_cmd::ManifestArgs),
    /// Generate a new box.yaml interactively
    Init(commands::init::InitArgs),
    /// Start a local REST API + dashboard server
    Serve(commands::serve::ServeArgs),
    /// Push or pull a box's state volume to/from a cloud remote (via rclone)
    Sync(commands::sync_cmd::SyncArgs),
}

fn print_banner() {
    const C: &str = "\x1b[36m";
    const D: &str = "\x1b[2;37m";
    const B: &str = "\x1b[1;36m";
    const W: &str = "\x1b[37m";
    const E: &str = "\x1b[1;97m";
    const R: &str = "\x1b[0m";

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

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Write a crash report to /tmp.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = format!("/tmp/agentbox-crash-{ts}.txt");
        let report = format!(
            "agentbox crash report\ntime: {ts}\n\n{info}\n\nbacktrace:\n{:?}\n",
            std::backtrace::Backtrace::capture()
        );
        if std::fs::write(&path, &report).is_ok() {
            eprintln!("\nagentbox crashed. Report written to: {path}");
            eprintln!("Please file an issue at https://github.com/fkeil/agentbox/issues");
        }
        eprintln!("{info}");
    }));
}

#[tokio::main]
async fn main() {
    // Crash diagnostics: capture panics and write a report file.
    install_crash_hook();

    // Structured logging: RUST_LOG controls verbosity (e.g. RUST_LOG=agentbox=debug).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .compact()
        .init();

    let args: Vec<String> = std::env::args().collect();
    let show_banner = args.len() == 1 || args.iter().any(|a| a == "--help" || a == "-h");
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
        Command::Profile(args) => commands::profile::run(args).await,
        Command::Manifest(args) => commands::manifest_cmd::run(args).await,
        Command::Init(args) => commands::init::run(args).await,
        Command::Serve(args) => commands::serve::run(args).await,
        Command::Sync(args) => commands::sync_cmd::run(args).await,
    };
    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
