use anyhow::{bail, Context};
use clap::Args;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct InitArgs {
    /// Output path for the generated box.yaml
    #[arg(long, short, default_value = "box.yaml")]
    pub output: PathBuf,
}

pub async fn run(args: InitArgs) -> anyhow::Result<()> {
    println!("agentbox init — interactive box.yaml generator");
    println!("(press Enter to accept defaults)\n");

    let agents = &[
        ("claude-code", "Claude Code (Anthropic)"),
        ("opencode", "OpenCode (multi-provider)"),
        ("pi", "Pi (15+ providers incl. Ollama)"),
        ("codex", "Codex (OpenAI)"),
    ];
    println!("Available agents:");
    for (i, (id, desc)) in agents.iter().enumerate() {
        println!("  {} — {id}: {desc}", i + 1);
    }
    let agent_input = prompt("Agent", "claude-code")?;
    let agent = agent_input.trim().to_lowercase();

    let folder_input = prompt("Workspace folder path", ".")?;
    let folder_path = PathBuf::from(folder_input.trim());
    if !folder_path.exists() {
        bail!(
            "folder `{}` does not exist — create it first",
            folder_path.display()
        );
    }

    let lifecycle_input = prompt("Lifecycle [ephemeral/persistent]", "ephemeral")?;
    let lifecycle = lifecycle_input.trim().to_lowercase();
    if lifecycle != "ephemeral" && lifecycle != "persistent" {
        bail!("lifecycle must be `ephemeral` or `persistent`");
    }

    let box_name = if lifecycle == "persistent" {
        let name = prompt("Box name (required for persistent)", "")?;
        let name = name.trim().to_string();
        if name.is_empty() {
            bail!("box name is required for persistent lifecycle");
        }
        Some(name)
    } else {
        None
    };

    let provider_default = if agent == "claude-code" {
        "anthropic"
    } else if agent == "codex" {
        "openai"
    } else {
        "anthropic"
    };
    println!("\nProvider types: anthropic, openai, openai-compatible");
    let provider_type_input = prompt("Provider type", provider_default)?;
    let provider_type = provider_type_input.trim().to_lowercase();

    let model_default = match provider_type.as_str() {
        "anthropic" => "claude-sonnet-4-6",
        "openai" => "gpt-4o",
        _ => "your-model-name",
    };
    let model_input = prompt("Model", model_default)?;
    let model = model_input.trim().to_string();

    let base_url = if provider_type == "openai-compatible" {
        let url = prompt("Base URL (e.g. http://localhost:11434/v1)", "")?;
        let url = url.trim().to_string();
        if url.is_empty() {
            bail!("base_url is required for openai-compatible providers");
        }
        Some(url)
    } else {
        None
    };

    let auth_hint = match provider_type.as_str() {
        "anthropic" => "${env:ANTHROPIC_API_KEY}",
        "openai" => "${env:OPENAI_API_KEY}",
        _ => "${env:API_KEY}",
    };
    let auth_input = prompt("Auth reference", auth_hint)?;
    let auth = auth_input.trim().to_string();

    // Warn if the env var is not set.
    if let Some(env_name) = auth
        .strip_prefix("${env:")
        .and_then(|s| s.strip_suffix('}'))
    {
        if std::env::var(env_name).is_err() {
            eprintln!("Warning: env var `{env_name}` is not currently set.");
        }
    }

    println!("\nEgress presets: open (default), block-local, provider-only, custom");
    println!("  open          — unrestricted internet access (default)");
    println!("  block-local   — deny access to 10.x, 172.16.x, 192.168.x");
    println!("  provider-only — only allow the AI provider (deny everything else)");
    println!("  custom        — specify allow/deny rules manually");
    let egress_input = prompt("Egress profile", "open")?;
    let egress_profile = egress_input.trim().to_lowercase();

    struct EgressOpts {
        default_deny: bool,
        deny: Option<String>,
        allow: Option<String>,
    }
    let egress = match egress_profile.as_str() {
        "block-local" => EgressOpts {
            default_deny: false,
            deny: Some("local-network".to_string()),
            allow: None,
        },
        "provider-only" => EgressOpts {
            default_deny: true,
            deny: None,
            allow: Some("provider host".to_string()),
        },
        "custom" => {
            let deny_input = prompt("Deny rules (space-separated, e.g. local-network 8.8.8.8)", "")?;
            let allow_input = prompt("Allow rules (space-separated, e.g. provider *.github.com)", "")?;
            EgressOpts {
                default_deny: false,
                deny: if deny_input.trim().is_empty() { None } else { Some(deny_input.trim().to_string()) },
                allow: if allow_input.trim().is_empty() { None } else { Some(allow_input.trim().to_string()) },
            }
        }
        _ => EgressOpts { default_deny: false, deny: None, allow: None },
    };

    let provider_name = provider_type.clone();
    let yaml = build_yaml(
        &agent,
        &folder_path,
        &lifecycle,
        box_name.as_deref(),
        &provider_name,
        &provider_type,
        &model,
        base_url.as_deref(),
        &auth,
        egress.default_deny,
        egress.deny.as_deref(),
        egress.allow.as_deref(),
    );

    if args.output.exists() {
        let overwrite = prompt(
            &format!("{} already exists. Overwrite? [y/N]", args.output.display()),
            "n",
        )?;
        if !overwrite.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    std::fs::write(&args.output, &yaml)
        .with_context(|| format!("failed to write `{}`", args.output.display()))?;

    println!("\nWritten to: {}", args.output.display());
    println!("Run with:  agentbox up --config {}", args.output.display());

    Ok(())
}

fn prompt(question: &str, default: &str) -> anyhow::Result<String> {
    if default.is_empty() {
        print!("{question}: ");
    } else {
        print!("{question} [{default}]: ");
    }
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("failed to read stdin")?;
    let line = line
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string();
    if line.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(line)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_yaml(
    agent: &str,
    folder_path: &Path,
    lifecycle: &str,
    box_name: Option<&str>,
    provider_name: &str,
    provider_type: &str,
    model: &str,
    base_url: Option<&str>,
    auth: &str,
    egress_default_deny: bool,
    egress_deny: Option<&str>,
    egress_allow: Option<&str>,
) -> String {
    let mut lines = vec![format!("agent: {agent}")];
    if let Some(name) = box_name {
        lines.push(format!("name: {name}"));
    }
    lines.push(format!("lifecycle: {lifecycle}"));
    lines.push(String::new());
    lines.push("folder:".to_string());
    lines.push(format!("  path: {}", folder_path.display()));
    lines.push(String::new());
    lines.push("provider:".to_string());
    lines.push(format!("  name: {provider_name}"));
    lines.push(format!("  type: {provider_type}"));
    lines.push(format!("  model: {model}"));
    if let Some(url) = base_url {
        lines.push(format!("  base_url: {url}"));
    }
    lines.push(format!("  auth: {auth}"));
    // Only emit egress block when non-default.
    if egress_default_deny || egress_deny.is_some() || egress_allow.is_some() {
        lines.push(String::new());
        lines.push("egress:".to_string());
        if egress_default_deny {
            lines.push("  default: deny".to_string());
        }
        if let Some(deny) = egress_deny {
            lines.push("  deny:".to_string());
            for rule in deny.split_whitespace() {
                lines.push(format!("    - {rule}"));
            }
        }
        if let Some(allow) = egress_allow {
            lines.push("  allow:".to_string());
            for rule in allow.split_whitespace() {
                lines.push(format!("    - {rule}"));
            }
        }
    }
    lines.push(String::new());
    lines.join("\n")
}
