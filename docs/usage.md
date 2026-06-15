# Agentbox — Usage Guide

Agentbox spins up an isolated container (Docker or Podman), installs a chosen AI coding agent inside it, and mounts a single folder from your host as the agent's only view of the filesystem. The agent cannot touch anything outside that folder.

Three frontends share the same engine: **CLI** (scriptable, CI-friendly), **TUI** (keyboard-driven terminal UI), and **GUI** (desktop app for non-CLI users).

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Installation](#2-installation)
3. [Core Concepts](#3-core-concepts)
4. [CLI Usage](#4-cli-usage)
5. [TUI Usage](#5-tui-usage)
6. [GUI Usage](#6-gui-usage)
7. [box.yaml Reference](#7-boxyaml-reference)
8. [Authentication Reference](#8-authentication-reference)
9. [Agents & Providers](#9-agents--providers)
10. [Egress Allowlist](#10-egress-allowlist)
11. [Snapshot / Diff Sync](#11-snapshot--diff-sync)
12. [Persistent Boxes](#12-persistent-boxes)
13. [OAuth (Subscription Auth)](#13-oauth-subscription-auth)
14. [Daemon Mode](#14-daemon-mode)
15. [Container and Image Management](#15-container-and-image-management)
16. [Container Backend (Docker / Podman / microVM)](#16-container-backend-docker--podman--microvm)
17. [Profiles (Presets)](#17-profiles-presets)
18. [Manifest Management](#18-manifest-management)
19. [Logging & Diagnostics](#19-logging--diagnostics)
20. [Troubleshooting](#20-troubleshooting)

---

## 1. Prerequisites

| Requirement | Notes |
|---|---|
| Docker Engine / Docker Desktop **or** Podman | Either backend works; auto-detected at startup |
| Rust + Cargo | Required to build from source |
| Linux or macOS | Windows: mostly works via WSL2 / Docker Desktop |
| For the GUI on Linux | `libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev` |

**Verify Docker:**
```bash
docker info
```

**Verify Podman** (Linux — enable the Docker-compatible socket first):
```bash
systemctl --user enable --now podman.socket
podman info
ls "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
```

To use Podman, add `backend: podman` to your `box.yaml`. Agentbox auto-detects the Podman socket when using `backend: auto` (the default), so you can also just start the socket and omit the field entirely:

```yaml
# box.yaml
backend: podman   # explicit
# — or —
backend: auto     # auto-detects Docker first, then Podman
```

---

## 2. Installation

### Build from source

```bash
git clone https://github.com/fkeil/agentBox
cd agentBox

# CLI + TUI (default workspace build)
cargo build --release

# The binary is at:
./target/release/agentbox
./target/release/agentbox-tui

# Optional: install to ~/.cargo/bin
cargo install --path cli
cargo install --path tui
```

### GUI (Linux)

```bash
sudo apt install libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev
cd gui/src-tauri
cargo run          # development
# or: cargo tauri dev  (requires cargo-tauri installed)
```

The GUI is a standalone Tauri project; it is not a workspace member (avoiding the webkit2gtk dependency for CI builds).

---

## 3. Core Concepts

| Concept | Description |
|---|---|
| **Box** | One isolated container + one agent + one folder. Defined by a `box.yaml`. |
| **Agent** | The AI coding tool (Claude Code, OpenCode, Pi, Codex). Described by a manifest YAML. |
| **Provider** | The model backend: `anthropic`, `openai`, or `openai-compatible` (Ollama, llama.cpp, etc.). |
| **Lifecycle** | `ephemeral` (container deleted on exit) or `persistent` (named, survives across sessions). |
| **Sync mode** | `mount` (live bind-mount, edits are immediate) or `snapshot` (copy-in → review diff → copy-out). |

---

## 4. CLI Usage

### Quick start

```bash
# 1. Create a box.yaml (see §7 for full reference)
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

# 2. Export your API key
export ANTHROPIC_API_KEY=sk-ant-...

# 3. Run
agentbox up --config box.yaml
```

The first run pulls the base image and installs the agent (~1–2 min). Subsequent runs reuse the cached image and start in seconds.

### Commands

```
agentbox up --config <path>     Launch a box from a box.yaml file
agentbox down <box-name>        Stop + remove a named persistent box (container + state volume)
agentbox list                   List all boxes (persistent + orphaned ephemeral containers)
agentbox kill <box-name>        Force-remove a container (no state volume deletion)
agentbox images                 List cached agent install images
agentbox images rm <agent-id>   Delete a specific cache image
agentbox images prune           Delete all cache images
agentbox agents                 List all available agents (manifests + built-ins)
agentbox attach <box-name>      Reconnect to a stopped/running persistent box
agentbox profile list           List saved profiles
agentbox profile save <name> --from <box.yaml>  Save a box.yaml as a named profile
agentbox profile run <name> --folder <path>  Launch a box from a saved profile
agentbox profile show <name>    Show a profile's settings
agentbox profile rm <name>      Delete a saved profile
agentbox manifest list          List all manifests (bundled + user-installed)
agentbox manifest add <source>  Install a manifest from a URL or local file
agentbox manifest rm <id>       Remove a user-installed manifest
```

### Examples

**Ephemeral box with local Ollama:**

```bash
cat > local.yaml << 'EOF'
agent: opencode
folder:
  path: ./my-project
lifecycle: ephemeral
provider:
  name: local-ollama
  type: openai-compatible
  base_url: http://host.docker.internal:11434/v1
  model: qwen2.5-coder:7b
  auth: none
EOF

agentbox up --config local.yaml
```

**Persistent named box:**

```bash
cat > dev.yaml << 'EOF'
agent: claude-code
name: dev-box
folder:
  path: ~/code/myapp
lifecycle: persistent
provider:
  name: anthropic
  type: anthropic
  model: claude-opus-4-8
  auth: ${env:ANTHROPIC_API_KEY}
EOF

agentbox up --config dev.yaml    # creates + enters box
agentbox attach dev-box          # reconnect later
agentbox down dev-box            # stop + remove when done
```

**Snapshot mode (review before write-back):**

```bash
cat > safe.yaml << 'EOF'
agent: claude-code
folder:
  path: ./important-project
  sync: snapshot
lifecycle: ephemeral
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: ${env:ANTHROPIC_API_KEY}
EOF

agentbox up --config safe.yaml
# Agent runs; when it exits a diff is printed.
# Use the TUI or GUI to review and approve changes.
```

**OAuth (subscription, no API key):**

```bash
cat > oauth.yaml << 'EOF'
agent: claude-code
folder:
  path: ./project
lifecycle: ephemeral
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: oauth
EOF

agentbox up --config oauth.yaml
# Claude Code will prompt you to open a URL and log in.
# The token is cached in a named volume; future runs skip the login.
```

**Egress allowlist (restrict outbound to provider only):**

```bash
cat > locked.yaml << 'EOF'
agent: claude-code
folder:
  path: ./project
network: allowlist
lifecycle: ephemeral
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: ${env:ANTHROPIC_API_KEY}
EOF

agentbox up --config locked.yaml
# Container can only reach api.anthropic.com + DNS.
# All other outbound traffic is dropped.
```

**Pi agent with custom Ollama models:**

```bash
cat > pi-local.yaml << 'EOF'
agent: pi
folder:
  path: ./project
lifecycle: ephemeral
provider:
  name: ollama
  type: openai-compatible
  base_url: http://host.docker.internal:11434/v1
  model: llama3.2:latest
  auth: none
  raw:
    providers:
      ollama:
        type: openai
        base_url: http://host.docker.internal:11434/v1
        models:
          - name: llama3.2:latest
          - name: codellama:7b
EOF

agentbox up --config pi-local.yaml
```

---

## 5. TUI Usage

Launch with:

```bash
agentbox-tui
```

### Home Screen

The home screen lists all boxes (persistent and orphaned ephemeral) with their agent, project name, and status.

- **●** green = running, **○** grey = stopped
- Orphaned ephemeral containers appear highlighted in red with a `⚠ orphaned` tag.

| Key | Action |
|---|---|
| `↑` / `↓` or `j` / `k` | Navigate box list |
| `Enter` | Open box detail / start "New box" wizard |
| `n` | Jump to "New box" wizard directly |
| `i` | Open Cache Images screen |
| `r` | Refresh box list |
| `q` | Quit |

### New Box Wizard (5 steps)

Press `n` or select **+ New box** to open the wizard.

**Step 1 — Agent**

Use `↑`/`↓` to highlight an agent, `Enter` to select.

**Step 2 — Folder & Project name**

- Type the host folder path (absolute or `~/…`).
- Press `Enter` to confirm the path (the project name field auto-fills from the folder basename).
- Press `Tab` to move to the **Project name** field and customise it.
- Press `Enter` again to advance to step 3.

The project name appears in window titles and the box list as "Agent - ProjectName".

**Step 3 — Lifecycle & Sync**

- `←` / `→` to toggle **ephemeral** / **persistent**.
- For persistent boxes a **Box name** field appears — `Tab` to reach it.
- `Enter` to advance.

**Step 4 — Provider**

- `←` / `→` to pick provider type (anthropic / openai / openai-compatible).
- `Tab` to move between fields (type, name, model, base URL, auth).
- For **Pi agent only**: a second screen appears for the `models.json` config. Paste valid JSON, press `F5` to continue (or leave empty to use the default provider via env vars).
- `Enter` on the last field to advance.

**Step 5 — Summary**

Review all settings. Press `Enter` to launch (opens the agent in the same terminal). Press `Esc` to go back.

### Box Detail Screen

Select a box and press `Enter` to see its detail view.

| Action | Key |
|---|---|
| Attach (reconnect) | `Enter` on **Attach** |
| Stop | `Enter` on **Stop** |
| Kill | `Enter` on **Kill** (orphaned containers only) |
| Remove | `Enter` on **Remove** |
| Back | `Esc` |

### Cache Images Screen

Press `i` from the Home screen to manage cached agent install images.

| Key | Action |
|---|---|
| `↑` / `↓` | Navigate image list |
| `d` or `Delete` | Delete selected cache image |
| `r` | Refresh image list |
| `Esc` | Back to Home |

### Color Themes

The TUI ships with 5 color themes. Press **Ctrl+T** anywhere to cycle through them:

| Theme | Description |
|---|---|
| **Dark** | Deep navy blue (default) |
| **Dracula** | Purple/pink on dark charcoal |
| **Nord** | Arctic blues and slate greys |
| **Catppuccin** | Mocha — warm pastel dark |
| **Gruvbox** | Earthy ambers and warm greens |

The active theme name is shown in the footer help bar.

### Global Keys

| Key | Action |
|---|---|
| `Ctrl+T` | Cycle color theme |
| `Esc` | Go back one screen |
| `q` | Quit (from Home or Summary) |

---

## 6. GUI Usage

### Starting the GUI

```bash
# From the project root (dev build):
cd gui/src-tauri && cargo run

# After cargo-tauri is installed:
cargo tauri dev
```

The GUI window opens at the Home screen.

### Dark / Light Mode

Click the **☀** button in the top-right corner of the title bar to toggle between dark and light mode. Your preference is saved to `localStorage` and restored automatically on next launch.

### Home Screen

Displays all boxes (persistent + orphaned ephemeral) as cards.

**Persistent box cards** show:
- Box name + lifecycle badge
- `Agent - ProjectName` label + folder path
- Status dot (● green = running, ○ grey = stopped)
- **Stop**, **Attach**, and **Remove** buttons (context-sensitive)

**Orphaned ephemeral container cards** show as red-tinted with a "⚠ orphaned" badge and a single **Kill** button to force-remove the container.

Press **New Box** to open the wizard. The **Cache Images** section appears below the box list (see §14).

### New Box Wizard (5 steps)

**Step 1 — Agent**: click an agent card to select it.

**Step 2 — Folder & Project name**:
- Paste or type the host folder path.
- The **Project name** field auto-suggests the folder basename as a placeholder. Type to override.

**Step 3 — Lifecycle & Sync**:
- Toggle **Ephemeral** / **Persistent**.
- For persistent: fill in the **Box name**.
- Toggle **Sync mode**: Mount (live) or Snapshot (review-before-write).

**Step 4 — Provider**:
- Pick provider type, fill in model, base URL (if openai-compatible), and auth.
- For agents that support OAuth (Claude Code, Codex): toggle between **API Key** and **OAuth / Subscription** auth modes. OAuth shows instructions for the in-container device-code flow.
- For **Pi**: a text area appears to enter the full `models.json` content (JSON). Leave empty to use the standard env-var auth path.

**Step 5 — Summary**: review, then click **Launch →**.

A terminal window opens running `agentbox up --config /tmp/agentbox-gui-launch.yaml`. The window title is set to `"Agent - ProjectName"`.

### Snapshot Diff Review

When a box uses `sync: snapshot` and the session ends, the GUI polls for the diff file. When it appears, a **Diff Review** screen opens automatically showing all changed files with unified diffs. Check the boxes next to files you want to apply, then click **Apply Selected**.

### Attaching to a Box

Click **Attach** on any stopped box card. A terminal window opens running `agentbox attach <box-name>`.

---

## 7. box.yaml Reference

```yaml
# Which agent to run. Must match a manifest id or built-in agent id.
# Available: claude-code, opencode, pi, codex
agent: claude-code

# Optional: shown in terminal window titles and the box list as "Agent - Project".
project_name: MyProject

# The single folder the agent can see (required).
folder:
  path: ./my-project          # absolute or relative to the yaml file
  sync: mount                 # mount (live) | snapshot (copy-in/diff/copy-out)

# Box lifecycle
lifecycle: ephemeral          # ephemeral (default) | persistent
name: my-box                  # required when lifecycle: persistent

# Model provider
provider:
  name: anthropic             # free label shown in logs
  type: anthropic             # anthropic | openai | openai-compatible
  model: claude-sonnet-4-6
  base_url: ~                 # null = provider default; required for openai-compatible
  auth: ${env:ANTHROPIC_API_KEY}  # see §8
  raw: {}                     # merged verbatim into agent native config

# Networking
network: open                 # open (default) | allowlist (see §10)

# Optional resource limits
resources:
  cpus: 2
  memory: 4g                  # units: g, m, k, b

# Extra env vars injected into the container (support same ${...} references as auth)
extra_env:
  SOME_TOKEN: ${env:SOME_TOKEN}
  LITERAL_VAR: "hello"
```

### Provider examples

```yaml
# Anthropic cloud
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: ${env:ANTHROPIC_API_KEY}

# OpenAI cloud
provider:
  name: openai
  type: openai
  model: gpt-4o
  auth: ${env:OPENAI_API_KEY}

# Local Ollama (running on host port 11434)
provider:
  name: local-ollama
  type: openai-compatible
  base_url: http://host.docker.internal:11434/v1
  model: llama3.2:latest
  auth: none

# Local llama.cpp server
provider:
  name: local-llamacpp
  type: openai-compatible
  base_url: http://host.docker.internal:8080/v1
  model: local-model
  auth: none

# Any LiteLLM / OpenRouter proxy
provider:
  name: openrouter
  type: openai-compatible
  base_url: https://openrouter.ai/api/v1
  model: anthropic/claude-sonnet-4-6
  auth: ${env:OPENROUTER_API_KEY}

# OAuth (no API key — agent handles login interactively)
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: oauth
```

---

## 8. Authentication Reference

Auth values in `box.yaml` are always **references**, never plaintext secrets. This makes configs safe to commit.

| Reference | Resolves to |
|---|---|
| `${env:VAR_NAME}` | Host environment variable |
| `${keychain:service/account}` | OS keychain (macOS Keychain / libsecret on Linux) |
| `${file:./path/to/secret.txt}` | Contents of a file (whitespace trimmed) |
| `none` | Empty string (for local providers that need no auth) |
| `oauth` | No key injected; agent handles OAuth device-code flow interactively |

### Examples

```yaml
# From host env var (most common)
auth: ${env:ANTHROPIC_API_KEY}

# From macOS Keychain
auth: ${keychain:anthropic/api-key}

# From a .gitignored file
auth: ${file:./secrets/anthropic.txt}

# No auth (local providers)
auth: none

# OAuth subscription (Claude Code, Codex)
auth: oauth
```

---

## 9. Agents & Providers

### Agent roster

| ID | Name | Mode | Providers | Notes |
|---|---|---|---|---|
| `claude-code` | Claude Code | session | anthropic | Anthropic's official CLI agent; OAuth supported |
| `opencode` | OpenCode | session | anthropic, openai, openai-compatible | Multi-provider cloud + local |
| `pi` | Pi | session | anthropic, openai, openai-compatible | 15+ providers via `models.json`; Ollama native |
| `codex` | Codex | session | openai, openai-compatible | OpenAI's CLI agent; OAuth supported |
| `hermes` | Hermes | **daemon** | openai, openai-compatible, anthropic | Persistent messaging agent; exposes port 8080 |

List available agents at runtime:

```bash
agentbox agents
```

### Adding a custom agent

Create `manifests/<id>.yaml` alongside the binary (or in the project root when running from source). The engine finds it automatically by walking up from the executable.

Minimal manifest skeleton:

```yaml
id: my-agent
display_name: My Agent
base_image: node:22-slim

install:
  method: npm          # npm | pip | script | binary
  packages:
    - my-agent-package
  # apt_deps: [curl]  # extra system packages installed before the agent
  # For script/binary methods:
  # url: https://example.com/install.sh
  # post_install: ["my-agent setup --non-interactive"]

supported_providers:
  - openai

auth:
  openai:
    api_key_env: OPENAI_API_KEY
    base_url_env: OPENAI_BASE_URL

# Static env vars always injected into the container for this agent.
# env:
#   MY_FLAG: "1"
#   SANDBOX: local

healthcheck: ["my-agent", "--version"]

launch:
  command: [my-agent]
  args_by_provider_type:
    openai: ["--model", "{{model}}"]

workdir: /workspace
```

**Install methods:**

| Method | Description |
|---|---|
| `npm` | `npm install -g <packages>` |
| `pip` | `pip install --quiet <packages>` |
| `script` | `curl -fsSL <url> \| NONINTERACTIVE=1 sh` — for agents with `install.sh` scripts |
| `binary` | `curl -fsSL <url>` → saved to `bin_path` (default `/usr/local/bin`) + `chmod +x` |

All methods accept `apt_deps` (system packages) and `post_install` (extra shell commands) as optional additions.

**Template variables** available in `launch.args` and `config.template`:

| Variable | Value |
|---|---|
| `{{model}}` | `provider.model` from box.yaml |
| `{{provider_name}}` | `provider.name` from box.yaml |
| `{{provider_slug}}` | `provider.name` slugified (lowercase, non-alnum → `-`) |
| `{{provider_type}}` | `anthropic` / `openai` / `openai-compatible` |
| `{{base_url}}` | `provider.base_url` or empty string |

### Pi agent — custom models

Pi natively supports multiple providers and models via `~/.pi/agent/models.json`. Set `raw:` in `box.yaml` to pass this file:

```yaml
provider:
  name: ollama
  type: openai-compatible
  base_url: http://host.docker.internal:11434/v1
  model: llama3.2:latest
  auth: none
  raw:
    providers:
      ollama:
        type: openai
        base_url: http://host.docker.internal:11434/v1
        models:
          - name: llama3.2:latest
          - name: codellama:7b
      anthropic-cloud:
        type: anthropic
        api_key: ""         # leave empty; set ANTHROPIC_API_KEY env instead
        models:
          - name: claude-sonnet-4-6
```

The JSON under `raw:` is written verbatim to `/root/.pi/agent/models.json` inside the container. The provider key (`ollama`, `anthropic-cloud`, etc.) must match the `provider.name` field.

---

## 10. Egress Allowlist

Set `network: allowlist` to restrict all outbound traffic from the container to the provider's API endpoint, DNS, and Docker bridge networks. Everything else is dropped.

```yaml
network: allowlist
```

**How it works:**

1. The container is created with `CAP_NET_ADMIN`.
2. After the agent is installed, iptables rules are applied inside the container.
3. Allowed: loopback, established connections, DNS (port 53), Docker bridge (`172.16.0.0/12`, `192.168.0.0/16`, `10.0.0.0/8`), and the resolved IPs of your provider's API hostname.
4. All other outbound traffic is `DROP`ped.

**Provider → allowed hostname:**

| Provider type | Resolved hostname |
|---|---|
| `anthropic` | `api.anthropic.com` |
| `openai` | `api.openai.com` |
| `openai-compatible` | hostname from `provider.base_url` |

> **Local providers** (`host.docker.internal`, `localhost`, `127.0.0.1`) are covered by the Docker bridge network rule; no extra resolution needed.

---

## 11. Snapshot / Diff Sync

Set `sync: snapshot` for a copy-in / review-diff / copy-out workflow:

```yaml
folder:
  path: ./my-project
  sync: snapshot
```

**What happens:**

1. At launch, the engine snapshots the host folder's file metadata (size + mtime).
2. The folder is **copied into the container** (no live bind-mount).
3. The agent runs against the copy.
4. On exit, the engine computes a unified diff between the original and the container state.
5. The diff is stored at `/tmp/agentbox-snapshot-<slug>.json`.
6. In TUI/GUI, a diff review screen shows all changed files. You approve which ones to write back.
7. **Conflict detection**: if any host file changed during the session (another process edited it), a warning is printed before the write-back. You can still apply the changes.

### Approve changes via CLI

After a snapshot run, the diff file is printed to stderr. To apply specific files:

```bash
# The GUI/TUI review UI handles this interactively.
# For scripting, use the apply-diff sub-command (available in core library).
```

### Why snapshot mode?

- Safer for long unattended runs: nothing hits your disk until you approve.
- Allows reviewing every change the agent made.
- Useful when the agent might delete or overwrite important files.

---

## 12. Persistent Boxes

Persistent boxes survive across sessions. Their state (installed tools, shell history, agent credential cache) lives in a named container volume (`agentbox-state-<name>`).

```yaml
lifecycle: persistent
name: my-dev-box
```

### Workflow

```bash
# First run: creates container, installs agent, attaches.
agentbox up --config dev.yaml

# Later: reconnect.
agentbox attach my-dev-box

# Or reconnect via the same config (detects existing box, skips install).
agentbox up --config dev.yaml

# Stop without removing.
# (Just exit the agent session — the box stops automatically.)

# List all boxes.
agentbox list

# Remove when done (deletes container + state volume).
agentbox down my-dev-box
```

### State volume

The state volume is mounted at `/root` inside the container. This means:
- Shell history (`~/.bash_history`) persists
- Agent credential caches (e.g. `~/.claude`) persist
- Any tools you install manually inside the box persist

---

## 13. OAuth (Subscription Auth)

Set `auth: oauth` to use in-container OAuth instead of an API key. The agent handles the device-code flow interactively; the token is cached in a named volume so future runs skip the login prompt.

```yaml
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: oauth
```

**Supported agents:**

| Agent | OAuth cache volume | Cache path |
|---|---|---|
| `claude-code` | `agentbox-oauth-claude-code` | `/root/.claude` |
| `codex` | `agentbox-oauth-codex` | `/root/.codex` |

**First run flow:**

1. The engine creates (or reuses) the named OAuth volume.
2. The agent starts and detects no cached token.
3. It prints a URL — open it in your host browser and log in.
4. The device-code flow completes; the token is saved to the volume.
5. On subsequent runs the volume is remounted and the token is found automatically.

> **Persistent boxes**: the OAuth cache path (`/root/.claude`, `/root/.codex`) is already under `/root`, which is covered by the state volume. No separate OAuth volume is mounted in this case.

---

## 14. Daemon Mode

Daemon-mode agents run as long-lived background services inside a persistent box rather than interactive sessions. They expose a local port (e.g. `localhost:8080`) and stay running until you explicitly stop them.

> Daemon mode is defined in the agent's manifest — you cannot turn a session agent into a daemon. Hermes (by Nous Research) is the built-in daemon agent.

### Requirements

- `lifecycle: persistent` — daemon agents require a named persistent box.
- `name:` must be set in `box.yaml`.

### Starting a daemon box

```bash
agentbox up --config hermes.yaml
```

The engine:
1. Creates and starts the container (if it doesn't exist).
2. Runs any non-interactive setup step defined in the manifest.
3. Launches the agent in the background (detached exec — it keeps running after the command exits).
4. Prints the bound ports and exits, leaving the daemon running.

Example output:
```
Starting Hermes daemon... started.

Daemon 'hermes-main' is running.
  localhost:8080 → container:8080
Stop with: agentbox down hermes-main
```

### Stopping a daemon box

```bash
agentbox down hermes-main
```

This stops and removes the container. The state volume (`agentbox-state-hermes-main`) is preserved so future runs skip reinstall.

### Checking daemon status

```bash
agentbox list
```

A running daemon box appears in the list. Run `agentbox up` again on the same config — if the daemon is already running, the command reports its status and exits immediately.

### Writing a daemon manifest

```yaml
id: my-daemon
display_name: My Daemon
base_image: ubuntu:22.04
install:
  method: script                        # or: npm | pip | binary
  url: https://example.com/install.sh   # for script/binary methods
  apt_deps: [curl, ca-certificates]     # system deps installed first
supported_providers:
  - openai
auth:
  openai:
    api_key_env: OPENAI_API_KEY
# Static env vars always injected for this agent.
env:
  DAEMON_SANDBOX: local
  DAEMON_NON_INTERACTIVE: "1"
launch:
  command: [my-daemon, run]
  args: []
workdir: /workspace
daemon:
  requires_lifecycle: persistent
  nested_sandbox: local   # injects HERMES_SANDBOX=local for hermes specifically
  setup:
    # Write a rendered config file to the container before the daemon starts.
    method: config_file    # or: exec | env
    config_path: /root/.my-daemon/config.json
    config_template: |
      {
        "provider": "{{PROVIDER_TYPE}}",
        "model": "{{MODEL}}",
        "api_key": "{{OPENAI_API_KEY}}",
        "sandbox": "local"
      }
    # For exec method: command: [my-daemon, setup, --non-interactive]
  ports:
    - container_port: 8080
      host_port: 8080
      optional: false
```

**Setup methods:**

| Method | Behaviour |
|---|---|
| `env` | All config via env vars already injected — no extra step |
| `exec` | Runs `command` inside the container non-interactively; must exit 0 |
| `config_file` | Renders `config_template` (with `{{ENV_VAR}}` substitution) and writes it to `config_path` |

**`config_file` template variables** — any env var injected into the container, plus these agentbox-provided ones:

| Variable | Value |
|---|---|
| `{{PROVIDER_TYPE}}` | `anthropic` / `openai` / `openai-compatible` |
| `{{MODEL}}` | `provider.model` from box.yaml |
| `{{API_KEY}}` | Resolved API key (empty if `auth: none`) |
| `{{BASE_URL}}` | `provider.base_url` or empty string |

---

## 15. Container and Image Management

### Orphaned containers

If a session crashes or the host process is killed mid-run, the container may be left behind instead of being cleaned up by the normal exit handler. These **orphaned containers** appear in `agentbox list` with lifecycle `ephemeral`.

**CLI:**
```bash
# Show all containers including orphaned ones
agentbox list

# Remove a specific orphaned container
agentbox kill claude-code-myproject

# The kill command is safe: it removes the container only, never a state volume
```

**TUI:** Orphaned containers appear in the home list with a `⚠ orphaned` marker in red. Press Enter to open detail, then select **Kill** to force-remove.

**GUI:** Orphaned containers show as red cards with a "Kill" button (in place of Stop/Attach/Remove).

### Cache image management

Agentbox commits the agent-installed image as `agentbox-cache-{agent}:latest` on first launch so subsequent launches skip the install step. You can list and delete these images if disk space is a concern, or to force a fresh reinstall.

**CLI:**
```bash
# List all cache images with sizes
agentbox images

# Remove one (will reinstall on next launch)
agentbox images rm claude-code

# Remove all
agentbox images prune
```

**TUI:** Press `i` from the Home screen to open the **Cache Images** screen. Use arrow keys to select an image and press `d` (or Delete) to remove it. Press `r` to refresh the list.

**GUI:** The **Cache Images** section appears below the box list on the Home screen. Each image shows its agent name, full Docker image tag, and size in MB. Click **Delete** to remove it.

---

## 16. Container Backend (Docker / Podman / microVM)

By default agentbox auto-detects the container backend: it checks `DOCKER_HOST`, then `/var/run/docker.sock`, then the Podman user socket. You can pin a specific backend in `box.yaml`:

```yaml
# box.yaml
backend: docker    # force Docker socket
backend: podman    # force Podman socket
backend: auto      # default — detect Docker first, then Podman
backend: microvm   # placeholder; returns an error until implemented
```

### Using Podman

Podman's Docker-compatible socket is supported via its REST API. To start the socket:

```bash
# Linux (systemd user session)
systemctl --user enable --now podman.socket

# Verify:
podman info
ls -la "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
```

Set `backend: podman` in your `box.yaml`, or set `DOCKER_HOST` to the socket path and use `backend: auto`:

```bash
export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/podman/podman.sock"
agentbox up --config box.yaml
```

### Auto-detection order

| Priority | Condition | Backend chosen |
|---|---|---|
| 1 | `DOCKER_HOST` env var is set | Connect to that URI |
| 2 | `/var/run/docker.sock` exists | Docker |
| 3 | `$XDG_RUNTIME_DIR/podman/podman.sock` exists | Podman |
| — | None of the above | Error |

---

## 17. Profiles (Presets)

A **profile** saves a `provider` + `agent` + `network` + `resources` + `extra_env` combination you want to reuse. Profiles are stored in `~/.config/agentbox/profiles/`.

### Save a profile

The easiest way is to save from an existing `box.yaml`:

```bash
agentbox profile save my-anthropic --from box.yaml
# Overwrite if it already exists:
agentbox profile save my-anthropic --from box.yaml --force
```

Or create the profile YAML directly in `~/.config/agentbox/profiles/my-profile.yaml`:

```yaml
name: my-profile
agent: claude-code
provider:
  name: anthropic
  type: anthropic
  model: claude-sonnet-4-6
  auth: ${env:ANTHROPIC_API_KEY}
network: open
lifecycle: ephemeral
```

### Use a profile

```bash
# Launch a box from the profile for a specific folder
agentbox profile run my-profile --folder ./my-project

# With a custom box name (required for persistent lifecycle)
agentbox profile run my-profile --folder ./my-project --box-name my-box

# Override lifecycle at invocation time
agentbox profile run my-profile --folder ./my-project --lifecycle persistent --box-name my-box
```

### List and manage profiles

```bash
agentbox profile list            # table of all saved profiles
agentbox profile show my-profile # show a profile's settings
agentbox profile rm my-profile   # delete a profile
```

---

## 18. Manifest Management

Agent manifests describe how to install and run an agent inside the container. Agentbox ships bundled manifests for Claude Code, OpenCode, Pi, Codex, and Hermes. You can also install community manifests from URLs or local files.

### List available agents/manifests

```bash
agentbox agents                  # lists all available agents
agentbox manifest list           # lists bundled + user-installed manifests with source
```

### Add a manifest

```bash
# From a URL (downloaded and saved to ~/.config/agentbox/manifests/)
agentbox manifest add https://example.com/my-agent.yaml

# From a local file
agentbox manifest add /path/to/my-agent.yaml

# Overwrite if a manifest with the same id already exists
agentbox manifest add --force https://example.com/my-agent.yaml
```

User-installed manifests take priority over bundled manifests with the same `id`. This lets you override a bundled manifest without modifying the agentbox binary.

### Remove a user-installed manifest

```bash
agentbox manifest rm my-agent-id
```

This only removes the user-installed copy. If a bundled manifest with the same `id` exists, it becomes active again.

### Writing a manifest

See `agentbox-spec.md §8` for the full schema. A minimal session-mode manifest:

```yaml
id: my-agent
display_name: My Agent
mode: session
base_image: node:22-slim
install:
  method: npm
  packages:
    - my-agent-npm-package
launch:
  command: [my-agent]
supported_providers: [anthropic, openai]
auth_env: MY_AGENT_API_KEY
config_file:
  path: /root/.my-agent/config.json
  template: |
    {
      "api_key": "{{MY_AGENT_API_KEY}}",
      "model": "{{MODEL}}"
    }
```

---

## 19. Logging & Diagnostics

### Structured logs

Agentbox uses structured logging via the `tracing` crate. Set `RUST_LOG` to control verbosity:

```bash
# Show info-level logs from agentbox crates
RUST_LOG=agentbox=info agentbox up --config box.yaml

# Full debug output (very verbose)
RUST_LOG=debug agentbox up --config box.yaml

# Only warnings (the default)
agentbox up --config box.yaml
```

The CLI writes logs to **stderr**. The TUI writes logs to **`/tmp/agentbox-tui.log`** to avoid corrupting the terminal UI.

```bash
# Follow TUI logs while the TUI is running
tail -f /tmp/agentbox-tui.log
```

### Crash reports

If agentbox crashes (Rust panic), a crash report is written to:

```
/tmp/agentbox-crash-<unix-timestamp>.txt
```

The file contains the panic message and a full stack backtrace. Include this file when filing a bug report at <https://github.com/fkeil/agentbox/issues>.

---

## 20. Troubleshooting

### Docker / Podman not running

```
Error: failed to connect to Docker
Hint: is Docker running? Check `docker info`
```

**Docker:** start Docker Desktop or run `sudo systemctl start docker`.

**Podman:** enable the socket if it isn't already:
```bash
systemctl --user enable --now podman.socket
```
If you want agentbox to find it automatically, make sure the socket path exists:
```bash
ls "${XDG_RUNTIME_DIR}/podman/podman.sock"
```
Or point `DOCKER_HOST` at it explicitly:
```bash
export DOCKER_HOST="unix://${XDG_RUNTIME_DIR}/podman/podman.sock"
```

### Agent install fails

```
agent install failed (exit=1):
...
Hint: check your network connection and try again
```

The base image needs internet access during install. If you are using `network: allowlist`, remember that the allowlist is applied **after** install, so install should still work. If it fails, check that the Docker daemon has internet access: `docker run --rm alpine ping -c 1 registry.npmjs.org`.

### Healthcheck failed

```
healthcheck failed for agent `codex` (exit 127):
sh: codex: not found
```

The agent binary was not found after install. This usually means the npm install failed silently. Delete the cached image to force a reinstall:

```bash
docker rmi agentbox-cache-codex:latest
```

### "unknown agent" error

```
Error: unknown agent `pi`
```

The manifests directory was not found at runtime. Make sure the `manifests/` folder is in the same directory as the binary, or in an ancestor directory. When running from source (`cargo run`), the engine walks up from the binary inside `target/` and finds the workspace-level `manifests/`.

### OAuth volume already exists with wrong data

```bash
docker volume rm agentbox-oauth-claude-code
```

Then run again — the volume is recreated and the agent will prompt for login.

### TUI: "JSON error: ..." when entering Pi models

The Pi models field expects a JSON object in the exact format Pi uses for `~/.pi/agent/models.json`. Make sure you paste **JSON**, not YAML. The error message shows the exact parse failure.

### Egress allowlist blocking the agent

If the agent cannot reach its API after allowlist is applied, check that DNS resolution of the provider hostname succeeds on the host:

```bash
getent hosts api.anthropic.com
```

If the host cannot resolve the hostname, neither can the allowlist rule (since IPs are resolved on the host before being applied inside the container).

For local providers: `host.docker.internal` is not DNS-resolved — it is covered by the Docker bridge network rule (`172.16.0.0/12` etc.) and does not need special handling.

### Snapshot conflict warning

```
Warning: the following files changed on the host during the session:
  ! src/main.rs
Applying agent changes anyway — review carefully.
```

This means the listed file was modified on your host filesystem while the agent was running in snapshot mode. The agent's version of the file will overwrite what's on disk for any file you approved. Check the diff carefully.

### Image cache is stale after agent update

```bash
# Remove the cached image for a specific agent:
docker rmi agentbox-cache-claude-code:latest

# Remove all agentbox cache images:
docker images 'agentbox-cache-*' -q | xargs docker rmi
```

The next run will reinstall from scratch and rebuild the cache.
