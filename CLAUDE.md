# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agentbox is a cross-platform CLI/TUI/GUI tool (Rust) that spins up an isolated container, installs a chosen AI coding agent, and mounts a single user-selected folder as the agent's only window onto the host. The spec lives in `agentbox-spec.md`.

## Build Commands

```bash
cargo build                          # build all workspace crates
cargo build -p agentbox-cli          # build only the CLI
cargo test                           # run all tests
cargo test -p agentbox-core          # test only core
cargo clippy -- -D warnings          # lint
cargo fmt --check                    # format check
```

## Workspace Structure

```
agentbox/
├─ core/         # engine — no UI deps
│   ├─ container/    # bollard-based lifecycle (create/start/exec/stop/rm)
│   ├─ manifest/     # parse + validate agent manifests (YAML)
│   ├─ config/       # parse + validate box.yaml
│   ├─ provider/     # provider resolution, host-gateway injection, compat checks
│   ├─ auth/         # secret reference resolution + injection
│   ├─ sync/         # mount + snapshot/diff engines
│   ├─ image/        # base image selection + agent install layering/caching
│   └─ engine.rs     # orchestrates a full "box run" end to end
├─ cli/          # Phase 1 frontend
├─ tui/          # Phase 3 frontend (Ratatui)
├─ gui/          # Phase 4 frontend (Tauri)
└─ manifests/    # bundled YAML agent manifests
```

`core` exposes a library; all frontends are thin clients of it. Never add UI deps to `core`.

## Key Architectural Decisions

**Container backend:** `core/container` is a trait so Docker and Podman are interchangeable. Podman is reached via its Docker-compatible socket.

**Agent manifests:** Adding an agent = adding a YAML file in `manifests/`. The manifest describes base image, install method, supported providers, auth env vars, native config template, and launch command. The engine validates `supported_providers` before touching Docker — fail fast at config time.

**Provider types:** `anthropic`, `openai`, `openai-compatible`. The engine maps unified fields (`type`, `model`, `base_url`, `auth`) into each agent's native config via the manifest template. `raw:` is merged verbatim for agent-specific knobs.

**Host gateway:** On Linux, the engine must inject `--add-host=host.docker.internal:host-gateway` so local providers (Ollama, llama.cpp) are reachable via the same `base_url` on all platforms.

**Secret references:** Auth values in `box.yaml` are always references (`${env:NAME}`, `${keychain:service/account}`, `${file:path}`, or `none`). The engine resolves them at runtime and injects the value as the env var named in the manifest. Secrets must never be written to disk or logged.

**Agent modes:** `session` (interactive, exits when done — default) vs `daemon` (long-lived service, forces `persistent` lifecycle). Hermes-Agent is the only current daemon agent.

## Configuration Schema

`box.yaml` — user-facing config. See `agentbox-spec.md §7` for the full schema.
Agent manifest YAML — see `agentbox-spec.md §8` for the full schema including daemon-specific fields.

## Phased Roadmap (current target: Phase 4)

**Phase 1 ✅ (MVP):** CLI + core. Docker only. Two hardcoded agents (Claude Code and OpenCode). Provider types: all three including local. Auth: `${env:...}` and `auth: none`. Sync: `mount` only. Lifecycle: `ephemeral` only. Commands: `agentbox up --config box.yaml`, `agentbox down`.

**Phase 2 ✅ (Manifest system):** YAML manifest loader + validator. `supported_providers` compat checking. Image caching. Three agents via manifests: Claude Code, OpenCode, Pi. `extra_env` in box.yaml for OAuth tokens. `agentbox agents` command.

**Phase 3 ✅ (TUI + persistent boxes):** Ratatui TUI (`agentbox-tui`): Home screen lists persistent boxes, Box detail (Attach/Stop/Remove), 5-step wizard (agent → folder → lifecycle → provider → summary). Persistent lifecycle: named containers, state volumes at `/root` keyed by box name. `agentbox list` command. `attach_box`, `stop_box`, `remove_box` engine calls.

**Phase 4:** Tauri GUI. Snapshot sync (copy-in / git-backed diff / approve-writeback).

**Phase 5:** Egress allowlist. In-container OAuth device-code flow. Codex manifest. Hermes-Agent (daemon mode, port mapping, non-interactive setup, `nested_sandbox: local`).

## Agent Roster

| Agent | Install | Mode | Supported providers |
|---|---|---|---|
| Claude Code | npm | session | anthropic |
| OpenCode | npm/binary | session | openai-compatible, anthropic, openai |
| Pi | npm | session | 15+, incl. Ollama |
| Codex | npm/binary | session | openai / openai-compatible |
| Hermes-Agent | install.sh | daemon | multi-model |

## Open Design Questions

- **Hermes non-interactive setup** (§16.3): `hermes setup` is interactive; need the config-file or env-driven equivalent before writing its manifest. Check Hermes docs/source before implementing.
- **Snapshot writeback conflicts** (§16.5): needs a conflict strategy when the host folder changes during a snapshot run (Phase 4 design detail).
- **Windows path translation** (§16.6): Docker Desktop/WSL2 quirks need a test pass in Phase 1.
