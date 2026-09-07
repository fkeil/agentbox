# Syncthing on TrueNAS SCALE, published through Traefik

Deployment bundle for TrueNAS SCALE 24.10+ (Electric Eel or newer), where Apps
run on plain Docker. Traefik terminates TLS on the same box and fronts the
Syncthing web UI; the Syncthing sync protocol bypasses Traefik and is published
directly to the host.

| File | Purpose |
|---|---|
| `docker-compose.yaml` | Traefik + Syncthing. Traefik's static config is in `command:` so that `.env` drives everything. |
| `.env.example` | Every deployment-specific value. Copy to `.env`. |
| `traefik/dynamic/syncthing.yaml` | Hot-reloaded middlewares: LAN allowlist + security headers. |

---

## 1. Free ports 80 and 443 first

**TrueNAS SCALE serves its own web interface on 80 and 443.** Traefik cannot
bind them until you move it, and the stack will fail to start with
`address already in use` if you skip this.

In the TrueNAS UI: **System Settings → General → GUI → Settings**

- Web Interface HTTP Port: `80` → `81`
- Web Interface HTTPS Port: `443` → `444`

Save and confirm. TrueNAS is then at `https://192.168.1.4:444`. Do this from a
session you can afford to lose — the UI reconnects on the new port.

## 2. Create datasets

**Datasets → Add Dataset**, or over SSH (replace `tank` with your pool):

```bash
zfs create -p tank/apps/traefik/acme
zfs create -p tank/apps/traefik/dynamic
zfs create -p tank/apps/syncthing/config
zfs create -p tank/sync
```

Give `tank/sync` the ownership that matches `PUID`/`PGID` in `.env` (`568:568`
is the `apps` account on SCALE):

```bash
chown -R 568:568 /mnt/tank/sync /mnt/tank/apps/syncthing /mnt/tank/apps/traefik
```

## 3. Install the files

Copy this directory to the NAS, then place the dynamic config where Traefik
expects it:

```bash
cp traefik/dynamic/syncthing.yaml /mnt/tank/apps/traefik/dynamic/
cp .env.example .env
$EDITOR .env          # see step 4
```

Leave `acme/` empty — Traefik creates `acme.json` with mode 600 on first run.
The directory (not the file) is mounted precisely so this works.

## 4. Fill in `.env`

At minimum: `SYNCTHING_HOST`, `ACME_EMAIL`, `ACME_DNS_PROVIDER`,
`CF_DNS_API_TOKEN`, `APPS_PATH`, `SYNC_DATA_PATH`.

Then add a DNS **A record** for `SYNCTHING_HOST` pointing at `192.168.1.4`. A
private address in a public zone is fine and intentional: DNS-01 validation
proves control of the zone by writing a TXT record, and Let's Encrypt never
connects to the host. Nothing is exposed to the internet by issuing this
certificate.

If you are not on Cloudflare, rename `CF_DNS_API_TOKEN` in **both** `.env` and
the `environment:` block of `docker-compose.yaml` to the variable your provider
expects.

## 5. Deploy

**Apps → Discover Apps → Custom App → Install via YAML**, paste
`docker-compose.yaml`, and supply the environment values. (Exact menu wording
shifts between point releases of Electric Eel.)

Or over SSH, from the directory holding `docker-compose.yaml` and `.env`:

```bash
docker compose up -d
docker compose logs -f traefik
```

Certificate issuance takes 30–120 seconds while the TXT record propagates.
You are looking for `Certificates obtained successfully` and no
`unable to generate a certificate`.

## 6. Set the admin password immediately

Open `https://<SYNCTHING_HOST>`.

**Syncthing ships with no credentials — the first person to reach the UI is the
administrator.** The LAN allowlist middleware is the only thing standing in
front of it until you do this:

**Actions → Settings → GUI →** set *GUI Authentication User* and *Password* →
**Save**.

---

## Connection details

| | |
|---|---|
| **Web UI** | `https://<SYNCTHING_HOST>` (HTTP on port 80 redirects) |
| **Direct UI fallback** | Not published. By design — reach it via Traefik, or `docker exec -it syncthing ...` if Traefik is down. |
| **Sync protocol (TCP)** | `192.168.1.4:22000` |
| **Sync protocol (QUIC)** | `192.168.1.4:22000/udp` |
| **Local discovery** | `21027/udp`, broadcast |
| **Credentials** | None until you set them in step 6. |
| **Device ID** | `docker exec syncthing syncthing --device-id` |
| **Config** | `/mnt/tank/apps/syncthing/config` |
| **Synced data** | `/mnt/tank/sync` → `/var/syncthing/data` in-container |

Peers add this NAS as `192.168.1.4:22000` plus the device ID above. Adding a
folder in the UI: use a path under `/var/syncthing/data`, which is the
container's view of `SYNC_DATA_PATH`.

### Syncing from outside the LAN

Nothing here opens the sync protocol to the internet. For remote peers, forward
`22000/tcp` and `22000/udp` on your router to `192.168.1.4`, or put both ends on
a WireGuard/Tailscale network and skip the forward. Do **not** forward the web
UI — remove the LAN allowlist only behind a VPN or a real authentication proxy.

### Local discovery caveat

`21027/udp` is a broadcast protocol and Docker's bridge network does not carry
broadcast traffic from the LAN into the container. Peers on `192.168.1.x` will
generally not auto-discover this device; add it by address instead. Global
discovery and relaying still work. If auto-discovery matters more than network
isolation, move the syncthing service to `network_mode: host` — but note that
this also exposes port 8384 on the LAN, so keep the GUI password set.

---

## Verifying

```bash
docker compose ps                       # both healthy
curl -sI https://<SYNCTHING_HOST> | head -1     # 200, trusted cert
docker exec syncthing syncthing --device-id
```

## Troubleshooting

**`address already in use` on Traefik** — step 1 was skipped; TrueNAS still
holds 80/443.

**404 from Traefik** — the router did not attach. Confirm `traefik.enable=true`
resolved and that both containers are on the `edge` network:
`docker inspect -f '{{json .Config.Labels}}' syncthing` and
`docker network inspect edge`.

**403 from Traefik** — the LAN allowlist rejected your source address. If you
are on `192.168.1.x` and still get 403, Docker is masquerading the client IP;
check what Traefik actually saw in `docker compose logs traefik`.

**Certificate not issued** — the DNS provider token lacks write access to the
zone, or the wrong environment variable name is set for the provider. Re-run
with `TRAEFIK_LOG_LEVEL=DEBUG` in `.env` and `docker compose up -d traefik`.

**Syncthing permission errors on folders** — `PUID`/`PGID` do not match the
owner of `SYNC_DATA_PATH`. Fix ownership rather than making the dataset
world-writable, which would break SMB/NFS ACLs on the same data.
