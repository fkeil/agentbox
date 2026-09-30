# Homelab AI + Obsidian — design state

Context handoff from a Claude Code web session (2026-09-30). Everything below
was decided in conversation and exists nowhere else. Read this before touching
`deploy/syncthing-truenas/`.

---

## Topology (established)

Three physical machines, all on `192.168.1.0/24`.

| Host | Role |
|---|---|
| **TrueNAS SCALE 24.10+** `192.168.1.4` | Bare metal, separate box. ZFS storage **and** the GPU (24GB+ VRAM). |
| **Proxmox** | Separate host. Already runs Traefik. Hosts the app-stack VM. |
| **Dell Pro Max / NVIDIA GB10** | ARM64, DGX OS. ComfyUI only. May be powered off; nothing depends on it. |

Phone is **iPhone/iPad** — this drives the vault sync decision below.

## Service placement

**TrueNAS** — GPU inference, reached over plain HTTP on the LAN:
- Ollama (14B–32B model; set a long `OLLAMA_KEEP_ALIVE`)
- faster-whisper (`large-v3`, ~3GB — coexists with Ollama on the same card)
- Syncthing (already deployed; **not** in the vault path — see below)

**Proxmox** — always-on and stateful, one Docker VM:
- Traefik (pre-existing)
- n8n — calls TrueNAS over HTTP for both inference steps
- CouchDB — **local storage, never NFS** (DB-over-NFS risks corruption); dump to TrueNAS on a schedule
- livesync-bridge — watches the vault mirror, replicates into CouchDB

**Dell GB10** — ComfyUI + Wan 2.2 only. No Traefik route, no n8n dependency.
Reach it over Tailscale. It is deliberately optional.

Vault mirror directory is NFS-mounted from TrueNAS into the VM: it is only
markdown, and living on ZFS makes the backup a snapshot task instead of a
script.

## Decisions and why

**Obsidian LiveSync + CouchDB, not Syncthing, for the vault.** Obsidian mobile
needs a local copy, so sync is mandatory. Syncthing has no native iOS client;
Möbius Sync manages ~1–2h of sync per day because iOS forbids background
daemons. LiveSync works properly on iOS and does real conflict resolution.

**Consequence:** the canonical vault lives inside CouchDB as chunked documents,
not as `.md` files. n8n cannot simply write a file into a folder. Hence
`vrtmrz/livesync-bridge`, which replicates a CouchDB vault against a filesystem
directory in both directions.

**LiveSync E2EE must be OFF.** It breaks the bridge's CouchDB↔storage peers —
it cannot read what it cannot decrypt. Acceptable here: CouchDB is on our own
hardware behind Tailscale, and the plaintext vault is on that box anyway. Pin a
bridge version and verify a round-trip before trusting it; the project has open
issues around chunk replication.

**Capture path has no GPU or Dell dependency.** Note ingestion must never fail
because a workstation is asleep.

**Telegram stays**, with the privacy cost understood and accepted: bot messages
are not E2EE, so voice notes transit Telegram's servers.

## Corrections owed to `deploy/syncthing-truenas/`

That bundle was written before we knew Traefik already existed on Proxmox. It
deploys a second, redundant Traefik. Not yet applied:

1. Delete the `traefik` service from `docker-compose.yaml`.
2. Revert the TrueNAS web UI to ports 80/443 — the move only existed to free
   them for the Traefik that is now leaving.
3. Publish Syncthing's `8384` on the host. It is currently unpublished on
   purpose, which a same-box Traefik could reach and a remote one cannot.
4. Replace the Syncthing Docker labels with a file-provider route on the
   Proxmox Traefik pointing at `192.168.1.4:8384`. Labels need a local socket.
5. Add `100.64.0.0/10` to the allowlist middleware — Tailscale uses CGNAT
   space, not RFC1918, so tailnet clients currently get a 403.

## Security posture

Exactly one public route: the n8n Telegram webhook path, IP-allowlisted to
`149.154.160.0/20` and `91.108.4.0/22`, with a `secret_token` on `setWebhook`
verified via the `X-Telegram-Bot-Api-Secret-Token` header.

Everything else — CouchDB, ComfyUI, Ollama, whisper, Syncthing, the n8n UI —
stays on LAN + Tailscale. Ollama and ComfyUI ship no authentication at all.

The **first node** of the Telegram workflow must check `message.from.id`
against the owner's ID and drop everything else. Without it, anyone who finds
the bot writes into the vault.

## Known gotchas

- CouchDB CORS must include `capacitor://localhost` (mobile Obsidian) as well
  as `app://obsidian.md` (desktop), with `credentials = true`. Omitting the
  mobile origin is the usual cause of "desktop syncs, phone does nothing".
  Also `single_node=true`, `require_valid_user=true` on both `[chttpd]` and
  `[chttpd_auth]`, and a raised `max_document_size`.
- Telegram `getFile` caps downloads at 20 MiB. Self-host `tdlib/telegram-bot-api`
  if that bites.
- n8n: pin `N8N_ENCRYPTION_KEY` or credentials break on restart.
- Single-writer rule: n8n only ever *creates* files in `_inbox/`, never edits
  existing notes.
- Deterministic filenames from timestamp + Telegram `message_id`, or retried
  workflows silently duplicate.
- ComfyUI on GB10 is sm_121/aarch64: **Conda will not work** — no wheels exist
  for that CUDA/arch combination. Use pip against
  `https://download.pytorch.org/whl/cu130`, or NVIDIA NGC containers.
- Syncthing and LiveSync must never manage the same directory.
- Neither Syncthing nor LiveSync is a backup; both replicate deletions. Snapshot
  the vault dataset.
- Wan 2.2 renders do not belong in the synced vault. Separate dataset, reference
  by path.

## Blocked on

`homelab-survey.sh` (this directory) has not been run. Needed before writing
configs, because the Traefik names used throughout the drafts — entrypoint
`websecure`, resolver `letsencrypt`, dynamic dir `/etc/traefik/dynamic` — are
**guesses** that must be replaced with whatever the existing Proxmox instance
actually uses. A route attached to the wrong resolver fails as a cert error.

Also unknown: real IPs, pool/dataset names, Proxmox storage and bridge names,
exact GPU model, whether Tailscale is installed anywhere, and the DNS zone.

## Next step

Run the survey on Proxmox and TrueNAS, then write: the corrected Syncthing
stack, the Proxmox Docker VM compose (n8n + CouchDB + bridge), the TrueNAS GPU
stack (Ollama + faster-whisper), Traefik file-provider routes including the
Telegram-restricted webhook rule, and the bridge config.
