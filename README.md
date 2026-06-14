# agentbox

[![CI](https://github.com/fkeil/agentBox/actions/workflows/ci.yml/badge.svg)](https://github.com/fkeil/agentBox/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

**Run AI coding agents in isolated containers — one folder, one agent, zero host exposure.**

Agentbox spins up a Docker container, installs your chosen AI coding agent inside it, and mounts a single host folder as the agent's only view of the filesystem. The agent cannot reach anything outside that folder or outside its allowed network endpoints. When the session ends, you review a diff and approve exactly what gets written back.

Three frontends, one engine: a scriptable **CLI**, a keyboard-driven **TUI**, and a desktop **GUI**.

---

## Quick start

```bash
# 1. Build
cargo build --release -p agentbox-cli

# 2. Create a box config
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
- **OAuth support** — In-container device-code flow; token cached in a Docker volume
- **CLI / TUI / GUI** — Same engine, pick your interface

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
| All | Docker Desktop or Docker Engine (Podman also works) · Rust + Cargo |
| Linux (GUI only) | `libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev` |

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

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
