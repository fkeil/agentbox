# Syncthing on TrueNAS SCALE

Syncthing only. Traefik lives on the Proxmox host — see
[`../homelab/`](../homelab/) for the routes and the rest of the stack.

> **Scope note.** Syncthing is no longer the Obsidian vault sync path. iOS has
> no background sync, so the vault moved to Obsidian LiveSync + CouchDB. Keep
> Syncthing for desktop and bulk files, and never point it at the vault
> directory — two sync systems on one folder manufacture conflicts.

## Install

**1. Create datasets** (Datasets → Add Dataset, or over SSH; replace `tank`):

```bash
zfs create -p tank/apps/syncthing/config
zfs create -p tank/sync
chown -R 568:568 /mnt/tank/sync /mnt/tank/apps/syncthing
```

Ownership must match `PUID`/`PGID` in `.env`.

**2. Configure and deploy:**

```bash
cp .env.example .env && $EDITOR .env
docker compose up -d
```

Or via Apps → Discover Apps → Custom App → Install via YAML.

TrueNAS keeps its default web UI ports — nothing here binds 80 or 443.

**3. Add the Traefik route.** Copy
`../homelab/traefik-routes/20-syncthing.yaml` into your Proxmox Traefik's
dynamic directory, substituting the placeholders. It is hot-reloaded.

**4. Set a GUI password immediately.** Syncthing ships with no credentials, and
8384 is now published on the LAN, so anything on your network can reach it
directly without passing Traefik's allowlist:

Actions → Settings → GUI → set user and password → Save.

## Connection details

| | |
|---|---|
| Web UI | `https://syncthing.<your-domain>` via Traefik, or `http://192.168.1.4:8384` direct |
| Sync protocol | `192.168.1.4:22000` tcp **and** udp |
| Local discovery | `21027/udp` |
| Device ID | `docker exec syncthing syncthing --device-id` |
| Config | `/mnt/tank/apps/syncthing/config` |
| Synced data | `/mnt/tank/sync` → `/var/syncthing/data` |

For remote peers, forward `22000/tcp` and `22000/udp`, or put both ends on
Tailscale. Never forward the web UI.

**Local discovery caveat:** `21027/udp` is broadcast, and Docker's bridge does
not carry LAN broadcast into containers, so peers generally will not
auto-discover this device — add it by address. `network_mode: host` fixes that
at the cost of isolation.

## Troubleshooting

**404 from Traefik** — the file-provider route is missing or still has
`__PLACEHOLDER__` values in it.

**403 from Traefik** — the allowlist rejected your source address. Tailscale
clients need `100.64.0.0/10`, which is in `10-middlewares.yaml`.

**Connection refused from Traefik** — 8384 is not published. Confirm with
`docker port syncthing`.

**Permission errors on folders** — `PUID`/`PGID` do not match the owner of
`SYNC_DATA_PATH`. Fix ownership rather than loosening the dataset, which would
break SMB/NFS ACLs on the same data.
