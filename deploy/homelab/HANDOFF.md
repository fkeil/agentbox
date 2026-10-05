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

## Pending decisions (2026-10-05) — the build is paused on these

- **Ollama moves to the GB10** (operator decision). This reverses "capture path
  has no Dell dependency": voice notes stop when the GB10 is off. Before
  building, either keep the GB10 always on, or keep Whisper on an always-on
  host so capture survives and only structuring waits.
- **TrueNAS may be retired** to save energy: storage absorbed into Proxmox
  (ZFS + Samba/NFS LXC) or onto a Raspberry Pi 5 with 2x4TB NVMe. A Pi cannot
  host a GPU (single PCIe lane, no slot). Recommended: Proxmox absorbs
  storage, Pi 5 becomes the off-box ZFS replication target.
- **Proxmox cleanup in progress** with `park-guests.sh` (reversible: graceful
  shutdown + onboot off, recorded in `/root/parked-guests.tsv`).

`RUNBOOK.md` hardwires TrueNAS as vault + GPU host. Do not execute it until
the layout above is settled; it will be rewritten for the chosen layout.

## Known RUNBOOK.md defects (fix when rewriting it)

Found in review; not yet patched. Blocking:

1. Nothing creates the vault dataset or its NFS export; Phase 2 `mount -a` fails.
2. Nothing creates DNS records for couch/n8n/syncthing/ollama; Gate 4a fails.
3. UID mismatch over NFS: n8n runs as 1000, dataset is 568. Phase 2's
   `.writetest` runs as root, so the gate does not catch it.
4. Phase 5's manual `setWebhook` + `secret_token` conflicts with n8n's Telegram
   Trigger, which registers its own webhook on activation (open n8n issues:
   URL silently dropped, 403s with a manual secret token). Pick one mechanism.

Gates that prove less than they claim:

5. Phase 3 tests file -> CouchDB only; CouchDB -> file (the phone's direction)
   is never checked.
6. `$USER`/`$PASS` are never set; `sample.wav` is never created.

Operational:

7. No CouchDB compaction. LiveSync keeps every chunk revision; databases grow
   many times the vault size. Needs scheduled `_compact` + chunk GC, or the
   VM disk fills and takes n8n and the bridge down.
8. No monitoring; every failure in the pipeline is silent.
9. No Docker log rotation.
10. No VM backup and no restore test.
11. Stale NFS mount after a TrueNAS reboot; `_netdev` only orders boot.
12. The Tailscale subnet router is a single point of failure for all remote
    access; non-iOS clients also need `--accept-routes`.

## Status of the deployed bundles

`deploy/syncthing-truenas/` has been corrected: the redundant Traefik service
is gone, `8384` is published so a remote Traefik can reach it, the Docker
labels are replaced by a file-provider route, and the README no longer tells
you to move the TrueNAS web UI off 80/443.

`deploy/homelab/` now carries the rest: the TrueNAS GPU stack, the Proxmox app
stack with the CouchDB CORS config and bridge config, and the Traefik routes.
See `README.md` there for build order.

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

`homelab-survey.sh` has not been run. Everything is written, but the Traefik
routes still carry `__TRAEFIK_ENTRYPOINT__`, `__TRAEFIK_CERTRESOLVER__`,
`__DOMAIN__` and `__APPSTACK_VM_IP__` placeholders, because the existing
Proxmox Traefik's real names are unknown. A route attached to a resolver that
does not exist fails as a certificate error.

Also unknown: pool/dataset names, Proxmox storage and bridge names, exact GPU
model, and whether Tailscale is installed anywhere.

## Next step

Run the survey, substitute the placeholders (`grep -rn '__[A-Z_]*__'`), then
follow the build order in `README.md`. The n8n workflow itself is described
there but not built — it is assembled in n8n's UI, not in a config file.
