# Homelab: private Obsidian vault + local AI

Read [`HANDOFF.md`](HANDOFF.md) first — it carries the topology, the decisions
and why they were made. [`RUNBOOK.md`](RUNBOOK.md) is the same build written as
instructions for an agent, with verification gates. This file is the build
order for a person.

| Directory | Runs on |
|---|---|
| [`truenas-gpu/`](truenas-gpu/) | TrueNAS `192.168.1.4` — Ollama + Whisper on the GPU |
| [`proxmox-appstack/`](proxmox-appstack/) | Docker VM on Proxmox — n8n, CouchDB, livesync-bridge |
| [`traefik-routes/`](traefik-routes/) | Your existing Proxmox Traefik's dynamic dir |
| [`../syncthing-truenas/`](../syncthing-truenas/) | TrueNAS — Syncthing (no longer the vault path) |

---

## Before anything: fill in the placeholders

Run [`homelab-survey.sh`](homelab-survey.sh) on Proxmox and TrueNAS. Every
value below is a **guess** until you do — a route pointing at a cert resolver
that does not exist fails as a certificate error, which is a miserable thing to
debug.

| Token | What it is | Where to find it |
|---|---|---|
| `__TRAEFIK_ENTRYPOINT__` | HTTPS entrypoint name | survey: Traefik static config, `--entrypoints.<name>.address=:443` |
| `__TRAEFIK_CERTRESOLVER__` | ACME resolver name | survey: `--certificatesresolvers.<name>.acme...` |
| `__DOMAIN__` | your DNS zone | existing router `Host()` rules |
| `__APPSTACK_VM_IP__` | the Docker VM's LAN address | after you create it |

```bash
grep -rn '__[A-Z_]*__' traefik-routes/          # find them all
sed -i 's/__DOMAIN__/home.example.com/g' traefik-routes/*.yaml   # substitute
```

`192.168.1.4` is hardcoded where TrueNAS is meant — it is already known.

## Build order

**1. TrueNAS GPU services.** Install the NVIDIA drivers first (Apps → Settings)
and confirm `nvidia-smi` works on the host.

```bash
cd truenas-gpu && cp .env.example .env && $EDITOR .env && docker compose up -d
docker exec ollama ollama pull qwen2.5:14b     # or your model of choice
curl -s http://192.168.1.4:11434/api/tags
curl -s http://192.168.1.4:8000/v1/models
```

**2. The Proxmox VM.** A plain Ubuntu VM with Docker. Mount the vault dataset
before starting the stack:

```bash
# /etc/fstab on the VM
192.168.1.4:/mnt/tank/vault  /mnt/vault  nfs  defaults,_netdev  0 0
```

```bash
cd proxmox-appstack
cp .env.example .env && $EDITOR .env
openssl rand -hex 32                    # N8N_ENCRYPTION_KEY
$EDITOR bridge/config.json              # password must match .env
chmod 600 bridge/config.json .env
docker compose up -d
```

**3. Create the CouchDB database** (LiveSync will not create it for you):

```bash
curl -X PUT http://$USER:$PASS@__APPSTACK_VM_IP__:5984/obsidian
curl -X PUT http://$USER:$PASS@__APPSTACK_VM_IP__:5984/_users
```

**4. Traefik routes.** Copy `traefik-routes/*.yaml` into the dynamic directory
the survey identified. Hot-reloaded — no restart.

**5. Obsidian LiveSync.** Install the plugin on desktop and iPhone, point both
at `https://couch.<domain>`, database `obsidian`. **Leave end-to-end encryption
off** — it breaks the bridge (see HANDOFF.md). Set up the desktop first, let it
populate, then connect the phone.

**6. Syncthing.** Apply the corrections in `../syncthing-truenas/`.

---

## Firewall: the published ports bypass Traefik

Traefik's allowlist only protects traffic that goes *through* Traefik. n8n on
`:5678`, CouchDB on `:5984`, Ollama on `:11434` and Whisper on `:8000` are all
published on the LAN and reachable directly. Ollama and Whisper have no
authentication whatsoever.

**On the app-stack VM**, restrict to the hosts that need it. Allow SSH first —
enabling `ufw` without it will lock you out of a remote machine:

```bash
ufw allow from 192.168.1.0/24 to any port 22 proto tcp   # do this FIRST
ufw allow from <TRAEFIK_HOST_IP> to any port 5678 proto tcp
ufw allow from <TRAEFIK_HOST_IP> to any port 5984 proto tcp
ufw default deny incoming
ufw enable
```

**On TrueNAS, do not do this.** SCALE ships no supported host firewall, and
hand-written iptables rules do not survive updates or middleware restarts — you
would be adding fragility, not security. Your options there are to restrict at
the router or on a VLAN, or to accept the LAN as the trust boundary for Ollama
and Whisper.

That is a real residual risk, not a solved one: any compromised device on your
network can drive both services. It is bounded — neither is reachable from the
internet — but worth knowing you are accepting it.

## The n8n workflow

Four things that are not optional:

1. **First node filters on `message.from.id`.** Anyone who finds your bot can
   message it. Without this check, strangers write into your vault.
2. **Set a `secret_token` on `setWebhook`** and verify the
   `X-Telegram-Bot-Api-Secret-Token` header. Telegram's IP allowlist is the
   outer gate; this is the inner one.
3. **Deterministic filenames** from timestamp + `message_id`, written only into
   `_inbox/`. n8n creates, never edits — that single-writer rule is what keeps
   LiveSync conflicts rare.
4. **An error workflow**, keeping the source audio until the `.md` is confirmed
   written. Otherwise a mid-run failure loses the note silently.

Shape: Telegram Trigger → filter sender → branch on voice/text → (voice:
`getFile`, download, POST to `http://192.168.1.4:8000/v1/audio/transcriptions`)
→ POST to `http://192.168.1.4:11434/api/generate` to structure and tag → write
`/vault/_inbox/<name>.md`.

Telegram's `getFile` refuses downloads over 20 MiB. Voice notes rarely reach
it; forwarded media will.

## Backup

Nothing here is a backup — LiveSync and Syncthing both replicate deletions.

- **Vault:** it is on ZFS, so a snapshot task on `tank/vault` covers it.
- **CouchDB:** on VM-local disk deliberately (databases over NFS risk
  corruption), so it needs its own job — replicate to a `.couch` dump on
  TrueNAS, or Proxmox Backup Server on the VM.
- **n8n:** credentials are encrypted with `N8N_ENCRYPTION_KEY`. Back up the key
  separately or the backup is undecryptable.
