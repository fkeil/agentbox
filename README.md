```
 █████╗  ██████╗ ███████╗███╗   ██╗████████╗██████╗  ██████╗ ██╗  ██╗
██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝██╔══██╗██╔═══██╗╚██╗██╔╝
███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║   ██████╔╝██║   ██║ ╚███╔╝ 
██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║   ██╔══██╗██║   ██║ ██╔██╗ 
██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║   ██████╔╝╚██████╔╝██╔╝ ██╗
╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝   ╚═════╝  ╚═════╝ ╚═╝  ╚═╝
```

# agentbox

[![CI](https://github.com/fkeil/agentBox/actions/workflows/ci.yml/badge.svg)](https://github.com/fkeil/agentBox/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)

**Run AI coding agents in isolated containers — one folder, one agent, zero host exposure.**

Agentbox spins up an isolated container (Docker or Podman), installs your chosen AI coding agent inside it, and mounts a single host folder as the agent's only view of the filesystem. The agent cannot reach anything outside that folder or outside its allowed network endpoints. When the session ends, you review a diff and approve exactly what gets written back.

Four interfaces, one engine: a scriptable **CLI**, a keyboard-driven **TUI**, a desktop **GUI**, and a **web dashboard** (`agentbox serve`).

---

## Quick start

```bash
# 1. Build
cargo build --release -p agentbox-cli

# 2. Generate a config interactively (recommended)
./target/release/agentbox init
# — or create one manually:
cat > box.yaml << 'EOF'
agent: claude-code
folder:
  path: ./my-project
lifecycle: ephemeral
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: ${env:ANTHROPIC_API_KEY}
EOF

# 3. Export your API key
export ANTHROPIC_API_KEY=sk-ant-...

# 4. Run
./target/release/agentbox up --config box.yaml
```

First run pulls the base image and installs the agent (~1–2 min). Subsequent runs reuse the cached image and start in seconds.

---

## Features

- **4 agents** — Claude Code, OpenCode, Pi, Codex (OpenAI)
- **All provider types** — Anthropic, OpenAI, any OpenAI-compatible endpoint (Ollama, llama.cpp, LiteLLM, OpenRouter)
- **Egress allowlist** — Drop-by-default iptables rules; only the provider API hostname gets through
- **Two sync modes** — `mount` (live bind-mount) or `snapshot` (copy-in → diff → review → copy-out)
- **Persistent boxes** — Named containers with state volumes survive across sessions
- **Docker or Podman** — auto-detected; pin with `backend: docker` / `backend: podman` in `box.yaml`
- **OAuth support** — In-container device-code flow; token cached in a named volume
- **Pre/post hooks** — Shell commands run on the host before/after each session
- **Extra mounts** — Additional host folders mounted read-only (or read-write) into the container
- **Remote boxes** — `--remote ssh://user@host` or `remote:` in `box.yaml` to run on a remote Docker host
- **Multi-box launch** — `agentbox up --config a.yaml --config b.yaml` to run several boxes in sequence
- **Web dashboard** — `agentbox serve` starts a REST API + browser UI on `localhost:7070`
- **Cloud sync** — `agentbox sync push/pull` backs up state volumes to any rclone remote (S3, GCS, SFTP, …)
- **Profile sharing** — `agentbox profile share/import` exports/imports profiles as base64 one-liners
- **Post-session summary** — git diff, egress log, and cost estimate printed after each session
- **OS notifications** — Desktop notification on session end (opt-in via `notifications: true`)
- **CLI / TUI / GUI / Web** — Same engine, pick your interface; terminal resizes propagate live

---

## Agents

| ID | Name | Providers |
|---|---|---|
| `claude-code` | Claude Code | anthropic |
| `opencode` | OpenCode | anthropic, openai, openai-compatible |
| `pi` | Pi | anthropic, openai, openai-compatible (15+ via models.json) |
| `codex` | Codex | openai, openai-compatible |

Add a custom agent by dropping a YAML manifest in `manifests/`. See [docs/usage.md §9](docs/usage.md) for the schema.

---

## Building

### Prerequisites

| Platform | Requirements |
|---|---|
| All | Docker Engine / Docker Desktop **or** Podman · Rust + Cargo |
| Linux (GUI only) | `libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev` |

**Docker:**
```bash
docker info          # verify Docker is running
```

**Podman** (Linux — enable the Docker-compatible socket):
```bash
systemctl --user enable --now podman.socket
podman info          # verify Podman is running
```

To use Podman, add `backend: podman` to your `box.yaml` (or set `DOCKER_HOST` and use the default `backend: auto`):
```yaml
backend: podman
```

### CLI + TUI

```bash
cargo build --release
# Binaries: ./target/release/agentbox   ./target/release/agentbox-tui
cargo install --path cli    # optional: install to ~/.cargo/bin
cargo install --path tui
```

### GUI (Tauri)

```bash
# Linux: sudo apt install libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev
cd gui/src-tauri && cargo run
# or: cargo install tauri-cli && cargo tauri dev
```

---

## Documentation

Full reference: **[docs/usage.md](docs/usage.md)**

Covers: all CLI commands, TUI navigation, GUI usage, `box.yaml` schema, auth reference syntax, egress allowlist, snapshot diff workflow, persistent boxes, OAuth, troubleshooting.

---

## License

Licensed under the [MIT License](LICENSE-MIT).
