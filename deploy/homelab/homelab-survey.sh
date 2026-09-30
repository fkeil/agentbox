#!/usr/bin/env bash
# Read-only homelab survey. Changes nothing; only reads config and status.
# Secrets (tokens, keys, passwords) are masked before output -- but skim the
# result before pasting it anywhere.
#
#   sudo bash homelab-survey.sh
#
# Run on the Proxmox host and on TrueNAS; it detects which it is on.

redact() {
  sed -E \
    -e 's/([A-Za-z0-9_-]*(TOKEN|KEY|PASSWORD|PASSWD|SECRET|APIKEY|AUTH)[A-Za-z0-9_-]*[=:"[:space:]]+)[^"[:space:],}]+/\1<REDACTED>/Ig' \
    -e 's/(password[[:space:]]*=[[:space:]]*)[^[:space:]]+/\1<REDACTED>/Ig' \
    -e 's/[A-Za-z0-9_-]{32,}/<REDACTED-LONG-STRING>/g' \
    -e 's/([A-Za-z0-9._%-]+)@([A-Za-z0-9.-]+)/<email>@\2/g'
}

h()   { printf '\n========== %s ==========\n' "$1"; }
have() { command -v "$1" >/dev/null 2>&1; }
try() { if have "$1"; then "$@" 2>&1 | head -60; else echo "(no $1)"; fi; }

h "IDENTITY"
echo "hostname: $(hostname)"
echo "os:       $(. /etc/os-release 2>/dev/null && echo "$PRETTY_NAME")"
[ -f /etc/version ] && echo "truenas:  $(cat /etc/version)"
have pveversion && echo "proxmox:  $(pveversion)"
echo "kernel:   $(uname -rm)"

h "NETWORK"
try ip -br -4 addr
echo "--- default route ---"; try ip route show default
echo "--- listening (0.0.0.0/:: only) ---"
have ss && ss -tlnp 2>/dev/null | grep -E '0\.0\.0\.0|\[::\]' | head -40 || echo "(no ss)"
echo "--- tailscale ---"
have tailscale && tailscale status 2>&1 | head -10 || echo "(not installed)"

h "GPU"
if have nvidia-smi; then
  nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv 2>&1
else
  echo "(no nvidia-smi)"
fi
have lspci && lspci -nn 2>/dev/null | grep -Ei 'vga|3d|display' || true

h "STORAGE"
try zpool list
echo "--- datasets (depth 2) ---"
have zfs && zfs list -o name,used,avail,mountpoint -d 2 2>/dev/null | head -40 || echo "(no zfs)"
echo "--- nfs exports ---"
[ -f /etc/exports ] && grep -vE '^\s*#|^\s*$' /etc/exports | head -20 || echo "(none)"

h "CONTAINERS"
if have docker; then
  docker ps --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}' 2>&1 | head -40
  echo "--- compose projects ---"
  docker ps --format '{{.Label "com.docker.compose.project.working_dir"}}' 2>/dev/null | sort -u | grep -v '^$'
else
  echo "(no docker)"
fi

# ---------- Proxmox ----------
if have qm; then
  h "PROXMOX GUESTS"
  qm list 2>&1
  echo "--- LXC ---"; try pct list
  echo "--- storage ---"; try pvesm status
  echo "--- bridges ---"
  grep -E 'iface|bridge-ports|address' /etc/network/interfaces 2>/dev/null | head -30
fi

# ---------- Traefik ----------
h "TRAEFIK"
TCONT=$(docker ps --format '{{.Names}}\t{{.Image}}' 2>/dev/null | grep -i traefik | cut -f1 | head -1)
if [ -n "${TCONT:-}" ]; then
  echo "container: $TCONT"
  echo "--- image ---"; docker inspect -f '{{.Config.Image}}' "$TCONT" 2>/dev/null
  echo "--- command (static config) ---"
  docker inspect -f '{{range .Config.Cmd}}{{println .}}{{end}}' "$TCONT" 2>/dev/null | redact
  echo "--- env ---"
  docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "$TCONT" 2>/dev/null | redact
  echo "--- mounts ---"
  docker inspect -f '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}' "$TCONT" 2>/dev/null
  echo "--- published ports ---"
  docker port "$TCONT" 2>/dev/null
else
  echo "no traefik container visible here"
  echo "--- searching for config files ---"
  find /etc /opt /srv /root /mnt -maxdepth 5 \
       \( -name 'traefik*.y*ml' -o -name 'traefik.toml' \) 2>/dev/null | head -10
fi

echo "--- static config file, if found ---"
for f in /etc/traefik/traefik.yaml /etc/traefik/traefik.yml /opt/traefik/traefik.yaml /srv/traefik/traefik.yml; do
  [ -f "$f" ] && { echo "### $f"; redact < "$f" | head -60; }
done

echo "--- dynamic config dir ---"
for d in /etc/traefik/dynamic /opt/traefik/dynamic /srv/traefik/dynamic; do
  [ -d "$d" ] && { echo "### $d"; ls -la "$d"; }
done

h "DONE"
echo "Review for anything sensitive before sharing."
