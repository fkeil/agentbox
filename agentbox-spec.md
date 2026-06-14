# Agentbox — Specification

*A cross-platform tool that spins up an isolated container, installs a chosen AI coding agent into it, auto-configures the agent from a declarative YAML file (including custom/local model providers), and mounts a single user-selected folder as the agent's only window onto the host — so the agent can run autonomously without host-wide access.*

> Status: Draft v0.1 · Working name: **Agentbox** (rename freely)

---

## 1. Vision & Problem Statement

Running an autonomous coding agent directly on your machine means handing it your entire filesystem, your shell, your SSH keys, and your credentials. The blast radius of a confused or malicious agent is your whole host.

Agentbox makes the safe path the easy path: describe what you want in one YAML file, and the tool creates a disposable, isolated environment with exactly one agent, exactly one folder mounted, and exactly the model provider you specified — local or cloud. You get autonomy without surrendering your host.

The design throughout follows one principle: **a sensible, safe default plus an opt-in path to full power.** This recurs in auth, sync, networking, and provider config.

---

## 2. Goals & Non-Goals

### Goals
- One declarative file (`box.yaml`) fully describes a runnable agent environment.
- Strong **filesystem** isolation of the host: the agent sees only the folder you attach.
- Support multiple agents via **declarative manifests** — adding an agent means adding a file, not changing code.
- Support **cloud and custom/local model providers** (Anthropic, OpenAI, OpenAI-compatible, and local Ollama / llama.cpp) from day one.
- Cross-platform: macOS, Linux, Windows.
- One shared engine, multiple thin frontends (CLI → TUI → GUI).
- Reproducible, shareable configs (no secrets baked into the file).

### Non-Goals (initially)
- VM-grade / hypervisor isolation (we accept shared-kernel containers — see Threat Model).
- Preventing data exfiltration over the network by default (egress is open; allowlist is an opt-in later phase).
- Multi-agent orchestration in a single box (locked: **one box = one agent**).
- Hosting or proxying models ourselves (we connect to providers; we don't run them).
- A marketplace/registry for manifests (possible far-future idea).

---

## 3. Threat Model & Security Philosophy

**What Agentbox protects:** the host filesystem and host environment. The agent runs as an unprivileged user inside a container with a dropped capability set, no host Docker socket, a read-only root filesystem where feasible, and only the attached folder bind-mounted in. The agent cannot read your home directory, your SSH keys, or anything outside the attached folder.

**What Agentbox does NOT protect by default:** data *inside* the box and the injected credentials, against network exfiltration. Because egress is open (a deliberate choice — agents need to fetch packages and hit APIs), a malicious agent or dependency could send the attached folder's contents or the API key somewhere. Mitigations exist and are opt-in:
- **Egress allowlist** (Phase 5) restricts outbound traffic to provider + registry endpoints.
- **Scoped, revocable API keys** keep credential blast radius small.
- **Local providers** (Ollama/llama.cpp) keep both model traffic and data on-machine.

**Residual risk we accept:** containers share the host kernel, so a kernel-level escape is theoretically possible. For the stated purpose ("don't let the agent roam my host"), this is an acceptable and clearly-communicated tradeoff. The architecture leaves room to swap the container backend for a microVM backend later without changing the user-facing model.

---

## 4. Locked Design Decisions

| Area | Decision |
|---|---|
| Isolation | Containers (Docker / Podman), shared-kernel |
| Host OS | Cross-platform (macOS, Linux, Windows) |
| Core language | Rust (`bollard` crate for the container API) |
| Frontends | CLI (MVP) → TUI (Ratatui) → GUI (Tauri), all over one core crate |
| Auth | Tiered: API key (default) → in-container OAuth (opt-in) → host-cred mount (discouraged) |
| Folder sync | Both: live bind-mount and snapshot/diff, user picks per run |
| Networking | Open egress by default; allowlist as opt-in hardening |
| Agents | Declarative manifests (plugin system) |
| Orchestration | One box = one agent |
| Providers | Unified core fields + `raw:` passthrough + custom/local endpoints |

---

## 5. Core Concepts

- **Box** — one isolated container instance defined by a `box.yaml`. Hosts exactly one agent.
- **Agent** — the AI coding tool installed into the box (e.g. Claude Code, Codex, OpenCode).
- **Manifest** — a declarative description of how to install, configure, authenticate, and launch a given agent. Manifests are what make the agent list extensible.
- **Provider** — the model backend the agent talks to (cloud or local), described by a type, endpoint, model, and auth.
- **Sync mode** — how the attached folder relates to the host: `mount` (live) or `snapshot` (copy-in / review-diff / copy-out).
- **Lifecycle** — `ephemeral` (fresh box, destroyed after) or `persistent` (named, reusable, retains state).

---

## 6. Architecture

A single Cargo workspace. The engine knows nothing about UI; every frontend is a thin client of `core`.

```
agentbox/
├─ core/        # the engine — no UI dependencies
│   ├─ container/    # bollard-based lifecycle (create/start/exec/stop/rm)
│   ├─ manifest/     # parse + validate agent manifests
│   ├─ config/       # parse + validate box.yaml
│   ├─ provider/     # provider resolution, host-gateway, compat checks
│   ├─ auth/         # secret resolution + injection
│   ├─ sync/         # mount + snapshot/diff engines
│   ├─ image/        # base image selection + agent install layering/caching
│   └─ engine.rs     # orchestrates a "box run" end to end
├─ cli/         # Phase 1 frontend
├─ tui/         # Phase 3 frontend (Ratatui)
├─ gui/         # Phase 4 frontend (Tauri)
└─ manifests/   # bundled agent manifests
```

**Core run loop (what `engine` does):** resolve config → load + validate manifest → check provider compatibility → select/build image with agent installed (cached) → create container (mounts, env, network, host-gateway) → write the rendered native agent config into the container → inject resolved secrets → launch the agent / attach the user → on exit, tear down (ephemeral) or stop (persistent); if snapshot mode, run the diff/writeback step.

**Container backend abstraction:** `core/container` is a trait so Docker and Podman (and, later, a microVM backend) are interchangeable. Podman is reached via its Docker-compatible socket.

---

## 7. Configuration Schema — `box.yaml`

The user-facing file. Designed so the common case is short and secrets are never inlined.

```yaml
# Which agent to run (matches a manifest id)
agent: claude-code

# The single folder the agent can see
folder:
  path: ./my-project
  sync: mount            # mount | snapshot

# Box lifecycle
lifecycle: ephemeral     # ephemeral | persistent
name: my-box             # required only when lifecycle: persistent

# Model provider — cloud or local
provider:
  name: anthropic              # free label
  type: anthropic              # anthropic | openai | openai-compatible
  model: claude-sonnet-4-5
  base_url: null               # null = provider default; set for custom/local
  auth: ${env:ANTHROPIC_API_KEY}   # reference, never plaintext (see §10)
  raw: {}                      # merged verbatim into the agent's native config

# Networking
network: open            # open | allowlist  (allowlist: Phase 5)

# Optional resource limits
resources:
  cpus: 2
  memory: 4g
```

### Custom / local provider examples (Phase 1)

Local Ollama running on the **host** machine:

```yaml
provider:
  name: local-ollama
  type: openai-compatible
  base_url: http://host.docker.internal:11434/v1   # core injects host-gateway on Linux
  model: qwen2.5-coder:7b
  auth: none
```

Local llama.cpp server:

```yaml
provider:
  name: local-llamacpp
  type: openai-compatible
  base_url: http://host.docker.internal:8080/v1
  model: local-model
  auth: none
```

**Host networking detail (handled by core, documented for users):** to let a container reach a service on the host, the engine maps `host.docker.internal` to the host gateway. This is automatic on Docker Desktop (macOS/Windows); on native Linux the engine adds `--add-host=host.docker.internal:host-gateway` so the same `base_url` works everywhere. Users write one `base_url` and it is portable across OSes.

---

## 8. Agent Manifest Schema

The contract that makes agents pluggable. Bundled manifests live in `manifests/`; users can supply their own.

```yaml
id: claude-code
display_name: Claude Code
version_pin: "@anthropic-ai/claude-code@latest"   # pin in real manifests

base_image: node:22-slim

install:
  method: npm            # npm | pip | binary | script
  package: "@anthropic-ai/claude-code"
  # for binary/script methods: url, checksum, post_install steps

# Which provider types this agent can actually talk to.
# The engine validates the user's provider against this list and
# fails fast with a clear message if incompatible.
supported_providers: [anthropic]      # e.g. opencode: [openai-compatible, anthropic, openai, ...]

auth:
  api_key:
    env: ANTHROPIC_API_KEY            # env var the agent reads
  base_url:
    env: ANTHROPIC_BASE_URL           # env var for custom endpoints
  oauth:
    supported: true                   # used in Phase 5
    device_code: true

config:
  path: /home/agent/.claude/settings.json   # where to write native config
  format: json                               # json | toml | yaml | env
  template: |                                # rendered from the provider block
    {
      "model": "{{model}}",
      "env": { "ANTHROPIC_BASE_URL": "{{base_url}}" }
    }

launch: ["claude"]                    # entrypoint inside the box
healthcheck: ["claude", "--version"]  # used to verify install succeeded
workdir: /workspace                   # where the attached folder mounts
```

**Compatibility matching:** because Claude Code speaks Anthropic-compatible and Ollama/llama.cpp speak OpenAI-compatible, `supported_providers` lets the engine reject mismatches up front (e.g. "claude-code cannot use an openai-compatible provider directly; use an agent that supports it, or route via an Anthropic-compatible proxy"). This turns a confusing runtime failure into a clear config-time error.

### 8.1 Agent Roster

Five agents are targeted. Four are **session** agents (run interactively in the box, then exit); Hermes is a **daemon** agent (a long-lived service). This `mode` distinction drives lifecycle, networking, and setup handling.

| Agent | Vendor | Install | Mode | Providers | Auth | Notes |
|---|---|---|---|---|---|---|
| Claude Code | Anthropic | npm | session | Anthropic-compatible | API key / OAuth | Cloud path reference |
| OpenCode | — | npm/binary | session | openai-compatible, anthropic, + many | API key / OAuth | Cloud + local path |
| Pi | Earendil | npm (`@earendil-works/pi-coding-agent`) or install.sh | session | 15+ incl. Anthropic, OpenAI, **Ollama**, OpenRouter, custom via `models.json` | API key / OAuth | print/JSON + RPC modes make non-interactive launch easy; recommends running in a container |
| Codex | OpenAI | npm/binary | session | OpenAI / openai-compatible | API key / OAuth | OAuth subscription auth aligns with Phase 5 |
| Hermes-Agent | Nous Research | `install.sh` -> `hermes setup` | **daemon** | multi-model | per-platform + model keys | Persistent service; messaging gateways; **its own sandbox backends** -- must use `local` backend inside our box (no Docker-in-Docker); needs persistent lifecycle + port mapping + non-interactive setup |

### 8.2 Manifest fields for daemon-style agents

Daemon agents (Hermes) need a few extra manifest fields beyond the session template above:

```yaml
id: hermes-agent
mode: daemon                 # session | daemon  (default: session)
requires_lifecycle: persistent   # daemon agents force persistent boxes
install:
  method: script
  url: https://hermes-agent.nousresearch.com/install.sh
setup:                       # non-interactive equivalent of `hermes setup`
  method: config_file        # config_file | env | exec
  # path/template/env to drive setup without interactive prompts
ports:                       # inbound mappings for gateway/webhooks
  - { container: 8080, host: 8080, optional: true }
nested_sandbox: local        # force the agent's own sandbox to 'local'
                             # since the Agentbox container already isolates
launch: ["hermes", "run"]
healthcheck: ["hermes", "--version"]
```

For **session** agents these fields are simply absent and the engine uses ephemeral-friendly defaults.

---

## 9. Provider Model

**Unified core fields** (`type`, `model`, `base_url`, `auth`) are normalized by the engine and mapped into each agent's native config via the manifest template. This keeps ~90% of configs portable across agents.

**`raw:` passthrough** is merged verbatim into the native config for agent-specific knobs (reasoning effort, thinking budget, tool config) the unified model doesn't cover. The abstraction never blocks power users.

**Provider types:**
- `anthropic` — Anthropic API or Anthropic-compatible endpoint.
- `openai` — OpenAI API.
- `openai-compatible` — any OpenAI-compatible endpoint, including local Ollama and llama.cpp.

**Local providers** are first-class in Phase 1 via `openai-compatible` + a `base_url` pointing at the host gateway. No special-casing beyond host networking and `auth: none`.

---

## 10. Authentication Model (Tiered)

1. **API key (default).** Resolved from a reference, never inlined:
   - `${env:NAME}` — host environment variable.
   - `${keychain:service/account}` — OS keychain (macOS Keychain, Windows Credential Manager, libsecret).
   - `${file:./secret.txt}` — gitignored file.
   - `none` — for local providers needing no auth.
   The resolved value is injected as the env var named in the manifest. Configs stay shareable and committable.
2. **In-container OAuth (opt-in, Phase 5).** Device-code flow surfaced to the user's host browser; token cached in a per-agent persistent volume so login survives ephemeral runs. Unlocks subscription plans.
3. **Host-cred mount (discouraged, Phase 5).** Read-only mount of the host's existing agent credentials, behind an explicit confirmation and a loud warning, for users who knowingly accept the weaker isolation.

---

## 11. Folder Sync Models

- **`mount` (live bind-mount).** The agent edits the real files directly. Best for fast iteration; mistakes hit disk immediately. *Available Phase 1.*
- **`snapshot` (copy-in / review-diff / copy-out).** The folder is copied into the box; on exit, a git-backed staging layer shows a diff and the user approves before anything is written back. Safer for unattended runs. *Available Phase 4.*

---

## 12. Networking Model

- **`open` (default).** Full outbound internet. Inbound: none.
- **`allowlist` (Phase 5).** Outbound restricted to provider endpoints + package registries, enforced at the container network layer. Opt-in hardening.
- **Host access** is limited to the explicitly mapped host gateway for local providers; the host Docker socket is never mounted.

---

## 13. Lifecycle Model

- **`ephemeral` (default).** Fresh container per run, destroyed on exit. Maximum reproducibility. *Phase 1.*
- **`persistent` (Phase 3).** A named box you reconnect to. Retains installed tools, shell history, agent credential cache, and (optionally) a named work volume. State lives in named Docker volumes keyed by box name.

---

## 14. Phased Roadmap

Each phase is independently shippable and ends with explicit exit criteria.

### Phase 1 — MVP: prove the core loop
**Goal:** From a `box.yaml`, create a Docker container, install one agent, configure it (cloud *or* local provider), mount a folder live, run the agent, tear down.

**Scope**
- CLI frontend + `core` engine.
- Docker backend only.
- Two agents: **Claude Code** (Anthropic / Anthropic-compatible) and **OpenCode** (natively `openai-compatible`). Claude Code proves the cloud path; OpenCode proves the cloud *and* local (Ollama / llama.cpp) paths end to end.
- Providers: `anthropic`, `openai`, **and `openai-compatible` including host-local Ollama / llama.cpp** with automatic host-gateway injection.
- Auth: API key via `${env:...}` references; `auth: none` for local.
- Sync: `mount` only.
- Lifecycle: `ephemeral` only.
- Network: `open` only.
- Agent wiring may be **hardcoded** for both agents (no manifest loader yet) to move fast.

**Deliverables:** `agentbox up --config box.yaml`, `agentbox down`, config parser + validator, host-gateway handling, native-config rendering + secret injection.

**Exit criteria:** A user can run Claude Code against an Anthropic key, *and* run OpenCode against either a cloud key or a local Ollama / llama.cpp instance, editing files in a mounted folder from inside the box — with the host filesystem outside that folder provably inaccessible.

### Phase 2 — Manifest system & second agent
**Goal:** Generalize the hardcoded agent into the declarative plugin system.

**Scope**
- Manifest schema, loader, and validator (§8).
- `supported_providers` compatibility checking.
- Convert both Phase-1 agents (Claude Code, OpenCode) from hardcoded wiring into manifests.
- Prove extensibility by adding **Pi** as a third agent via a manifest file *only*, with no core changes. Pi is ideal here: npm-installable, natively multi-provider including Ollama (reinforcing the local path), and its RPC/print modes make non-interactive launch clean.
- `supported_providers` compatibility checking exercised across the three agents.
- Image layering + caching (base image + agent install layer) for fast repeat runs.
- Manifest version pinning.

**Exit criteria:** Adding an agent requires only a new manifest file; all three agents (Claude Code, OpenCode, Pi) run against cloud and, where compatible, local providers.

### Phase 3 — TUI & persistent boxes
**Goal:** Interactive use and reusable environments.

**Scope**
- Ratatui TUI over `core`: pick agent, edit/select config, view logs, start/stop, attach.
- `persistent` lifecycle: named boxes, state volumes, reconnect, list/remove.
- Per-agent credential/state caching.

**Exit criteria:** A named box survives across sessions retaining installed tools and history; the TUI can manage its full lifecycle.

### Phase 4 — GUI & snapshot/diff sync
**Goal:** Mainstream-friendly desktop app and review-before-writeback.

**Scope**
- Tauri GUI (Rust backend + web frontend) over the same `core`.
- `snapshot` sync: copy-in, git-backed staging, visual diff, approve/discard writeback.
- Config editing with schema-aware validation in the GUI.

**Exit criteria:** A non-CLI user can configure and run a box from the GUI and review a diff before any host file is changed.

### Phase 5 — Hardening & subscription auth
**Goal:** Close the security seams and unlock subscription-based auth.

**Scope**
- `allowlist` egress mode enforced at the network layer.
- In-container OAuth device-code flow with persistent token volumes.
- Discouraged read-only host-cred mount with explicit warning.
- **Codex** manifest (API-key works earlier; OAuth/subscription auth lands here).
- **Hermes-Agent** support — the daemon-agent path. This is the most involved addition because Hermes is not session-based: it requires the `daemon` manifest mode (§8.2), forces `persistent` lifecycle (built in Phase 3), needs inbound port mapping for its gateway, a non-interactive driver for `hermes setup`, and `nested_sandbox: local` so it uses its own local backend rather than attempting Docker-in-Docker. Messaging-platform credentials (Telegram, Discord, etc.) flow through the same secret-reference system as model keys.

**Exit criteria:** A user can run a session agent on a subscription plan with no API key and lock egress to an allowlist; and can run Hermes-Agent as a persistent daemon box with at least one messaging gateway configured non-interactively.

### Phase 6 — Complete / polish (optional)
**Goal:** Production-grade ergonomics.

**Scope**
- Podman backend parity.
- Profiles / presets (named reusable provider + agent combos).
- Telemetry-free usage stats opt-in, structured logs, crash diagnostics.
- Manifest sharing/import (file or URL), signature verification.
- Optional microVM backend behind the same container trait (path to stronger isolation without a UX change).

---

## 15. Cross-Cutting Concerns
- **Config validation:** strict schema with actionable error messages; validate provider/agent compatibility before touching Docker.
- **Secrets:** never written to disk by the engine; resolved at runtime, injected as env, scrubbed from logs.
- **Reproducibility:** pin agent versions and base image digests in manifests.
- **Maintenance reality:** agents' native config formats drift across versions; manifests need pinning and periodic upkeep. Budget for it — treat manifests as living files.
- **Logging:** structured logs from `core`; frontends choose how to surface them.
- **Errors:** fail fast and legibly at config time rather than deep inside a container run.

---

## 16. Risks & Open Questions
1. **Local provider + Claude Code mismatch — RESOLVED.** Claude Code is Anthropic-compatible; Ollama/llama.cpp are OpenAI-compatible. Decision: ship **OpenCode** alongside Claude Code in Phase 1 so the local-provider path is demonstrable end to end without a translation proxy.
2. **Agent identities — RESOLVED.** Pi = Earendil's terminal coding agent (`@earendil-works/pi-coding-agent`), session-mode, natively multi-provider incl. Ollama. Hermes = Nous Research's persistent daemon agent (`install.sh` + `hermes setup`), daemon-mode. Both slotted: Pi in Phase 2, Hermes in Phase 5.
3. **Hermes non-interactive setup — TO CONFIRM.** `hermes setup` is interactive; we need its config-file or env-driven equivalent to auto-configure it from `box.yaml`. Requires reading Hermes docs/source before writing its manifest.
4. **Hermes gateway/inbound exposure.** Messaging gateways may need inbound webhooks or outbound long-polling; the `ports` manifest field covers mapping, but each platform's connectivity model should be checked.
5. **Snapshot writeback conflicts.** If the host folder changes during a snapshot run, the diff/writeback needs a conflict strategy (Phase 4 design detail).
6. **Windows specifics.** Path translation and Docker Desktop/WSL2 backend quirks need a test pass in Phase 1.
7. **Provider auth for local but networked endpoints.** Some local gateways (e.g. LiteLLM) do require a key — `auth` already supports this; just flagging it's not always `none`.

---

## 17. Future Ideas (out of current scope)
- MicroVM backend for VM-grade isolation, same UX.
- Manifest registry / marketplace.
- Multi-box orchestration despite the one-box-one-agent default (e.g. fan-out runs).
- Snapshotting box state for replay/debugging.
- Policy files (org-wide defaults for network/auth/resources).
