# syncboy runbook — DNS at 192.168.1.7 + off-box PBS copy

For an agent on a machine on the home LAN (it needs SSH to the Pi and to the
existing PBS). Decided 2026-10-13:

- syncboy (Raspberry Pi, 2 TB SSD, now `192.168.1.236`) is **cleaned in place**,
  not reflashed.
- It takes over **`192.168.1.7`**, the address of the Pi-hole that died, so
  clients already pointed there resume working without touching the router.
- It becomes a **second PBS** that *pulls* copies from the PBS you already run:
  the off-box copy of your existing backups.

## Rules

- **Show before you remove.** Phase 1 lists everything; the operator confirms
  each removal. Never wipe a disk or partition without showing `lsblk` and
  getting an explicit yes for that device.
- On the **existing PBS**, only read, plus create the one sync user in Phase 5
  after the operator approves. Never prune, delete or reconfigure it.
- Pull, never push: the main PBS must not hold credentials that can delete
  syncboy's copies. That separation is what makes this a backup.
- Stop at every GATE. A gate passes only when shown passing.

---

## Phase 0 — Identify

```bash
ssh <user>@192.168.1.236
tr -d '\0' < /proc/device-tree/model; echo
. /etc/os-release; echo "$PRETTY_NAME"; getconf PAGESIZE; lsblk -f; df -h
```

On the existing PBS: `proxmox-backup-manager versions` and
`proxmox-backup-manager datastore list`, plus the size of each datastore.

**GATE 0** — report: board model, OS release, page size, disk layout (is the
OS on the SSD or on an SD card?), existing PBS major version, and the total
size of what will be synced. **If that exceeds ~1.8 TB, stop**: the operator
chooses which datastores or namespaces to copy.

**Version match:** PBS 4 needs Debian trixie, PBS 3 bookworm. If the Pi's OS
is a major release behind the existing PBS, report it. An in-place
bookworm→trixie upgrade is the operator's call, not the agent's.

## Phase 1 — Inventory and clean

List, then remove **only what the operator confirms**:

```bash
systemctl list-units --type=service --state=running --no-pager
docker ps -a 2>/dev/null; docker volume ls 2>/dev/null
apt-mark showmanual
crontab -l; sudo crontab -l; ls /etc/cron.d
```

Typical removals: containers, images and volumes (`docker rm`, `docker volume
rm`), old services (`systemctl disable --now`, then `apt purge`), the dead
Syncthing/Pi-hole remnants. Keep: ssh, networking, NetworkManager.

```bash
sudo apt update && sudo apt full-upgrade -y && sudo apt autoremove --purge -y
sudo reboot
```

**GATE 1** — after the reboot: SSH works, `apt list --upgradable` is empty,
and nothing the operator asked to remove is still running.

## Phase 2 — Take over 192.168.1.7

Confirm nothing answers on .7 first — the dead Pi-hole may not be fully dead:

```bash
IF=$(ip -4 route show default | awk '{print $5; exit}')
GW=$(ip -4 route show default | awk '{print $3; exit}')
sudo arping -D -c 3 -I "$IF" 192.168.1.7 && echo "free" || echo "IN USE - stop"
CON=$(nmcli -g NAME,DEVICE con show --active | awk -F: -v d="$IF" '$2==d{print $1}')
echo "interface=$IF gateway=$GW connection=$CON"
```

If free, set it. This drops the SSH session; reconnect at `.7`:

```bash
sudo nmcli con mod "$CON" ipv4.method manual ipv4.addresses 192.168.1.7/24 \
     ipv4.gateway "$GW" ipv4.dns "1.1.1.1 9.9.9.9"
sudo nmcli con up "$CON"
```

`ipv4.dns` stays external until Pi-hole is up, so updates keep working.

**Operator:** make sure `.7` is reserved or outside the router's DHCP pool,
or the router may hand it to something else later.

**GATE 2** — `ssh <user>@192.168.1.7` works and `.236` no longer answers.

## Phase 3 — Pi-hole v6 (Docker)

Docker, because the v6 bare-metal installer is interactive and its unattended
mode is unreliable. Host networking so it serves DNS on port 53 directly.

```bash
curl -fsSL https://get.docker.com | sh
sudo mkdir -p /opt/pihole && cd /opt/pihole
```

`/opt/pihole/compose.yaml`:

```yaml
services:
  pihole:
    image: pihole/pihole:latest   # pin to the current tag once it works
    container_name: pihole
    network_mode: host
    restart: unless-stopped
    environment:
      TZ: Europe/Lisbon
      FTLCONF_webserver_api_password: "${PIHOLE_PASSWORD}"
      FTLCONF_dns_upstreams: "9.9.9.9;1.1.1.1"
      FTLCONF_dns_listeningMode: "all"
      FTLCONF_webserver_port: "8080"   # leave 80/443 free
    volumes:
      - ./etc-pihole:/etc/pihole
```

Check these variable names against the image's README before starting; the
v6 image renamed its settings and the README is authoritative.

```bash
echo "PIHOLE_PASSWORD=$(openssl rand -base64 18)" | sudo tee .env   # give to operator
sudo chmod 600 .env && sudo docker compose up -d
```

Then point the Pi itself at Pi-hole: `nmcli con mod "$CON" ipv4.dns 127.0.0.1`.

If the operator has a **Teleporter export** from the old Pi-hole, import it in
the web UI (Settings → Teleporter) to get the old blocklists back.

**GATE 3** — from a *different* machine:

```bash
dig @192.168.1.7 example.com +short        # returns an address
dig @192.168.1.7 doubleclick.net +short    # returns 0.0.0.0 (blocked)
```

Web UI at `http://192.168.1.7:8080/admin`.

## Phase 4 — PBS on the Pi (community build)

Unofficial: Proxmox ships no PBS for Raspberry Pi. Build source:
<https://github.com/wofferl/proxmox-backup-arm64>. Follow its README; prefer
its release packages matching the OS (trixie → PBS 4, bookworm → PBS 3), and
match the existing PBS's major version.

**Pi 5 only:** the default kernel uses 16k pages, which PBS cannot use.

```bash
echo "kernel=kernel8.img" | sudo tee -a /boot/firmware/config.txt && sudo reboot
getconf PAGESIZE    # must print 4096
```

Datastore on the SSD. If the OS lives on the SSD, use a directory; if the SSD
is a separate disk, format it only after the operator confirms the device:

```bash
sudo mkdir -p /mnt/pbs/syncboy
sudo proxmox-backup-manager datastore create syncboy /mnt/pbs/syncboy
```

**GATE 4** — `https://192.168.1.7:8007` loads, and the `syncboy` datastore
shows the expected capacity.

## Phase 5 — Pull from the existing PBS

On the **existing** PBS (operator approves first), create a read-only user:

```bash
proxmox-backup-manager user create sync-syncboy@pbs --password '<generated>'
proxmox-backup-manager acl update /datastore/<name> DatastoreReader --auth-id sync-syncboy@pbs
proxmox-backup-manager cert info | grep Fingerprint
```

On **syncboy**: add the remote (Configuration → Remotes, with that
fingerprint), then a **pull** sync job into `syncboy`, scheduled after the
main backup window. Add a prune job and GC schedule so 2 TB doesn't fill:
for example keep-daily 7, keep-weekly 4, keep-monthly 6.

Client-side encrypted backups sync as-is and stay encrypted. The encryption
key still has to be stored somewhere other than both PBS hosts.

**GATE 5** — the first sync completes, then **restore one guest from
syncboy's copy** to a new VMID on Proxmox, boot it, and delete it. A sync that
has never been restored from is not yet a backup.

## Done

Record in `HANDOFF.md`: syncboy is at `.7`, the PBS versions on both ends,
datastore size and retention, and the date of the restore test.
