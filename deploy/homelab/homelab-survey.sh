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

# ---------- Proxmox backups ----------
if have pvesh; then
  h "PROXMOX BACKUP COVERAGE"
  echo "--- guests in NO backup job (the direct answer) ---"
  pvesh get /cluster/backup-info/not-backed-up 2>&1
  echo "--- scheduled backup jobs ---"
  if [ -s /etc/pve/jobs.cfg ]; then redact < /etc/pve/jobs.cfg; else echo "(no /etc/pve/jobs.cfg)"; fi
  [ -s /etc/pve/vzdump.cron ] && { echo "--- legacy vzdump.cron ---"; grep -v '^#' /etc/pve/vzdump.cron; }
  echo "--- backup storages ---"
  pvesm status --content backup 2>&1
  echo "--- newest backup per guest (VMID  volume) ---"
  # Volume names embed the timestamp, so lexical order is chronological per VMID.
  for st in $(pvesm status --content backup 2>/dev/null | awk 'NR>1 {print $1}'); do
    pvesm list "$st" --content backup 2>/dev/null | awk 'NR>1'
  done | sort | awk '{last[$NF]=$1} END {for (v in last) print v, last[v]}' | sort -n
  echo "--- recent backup task results ---"
  pvesh get "/nodes/$(hostname)/tasks" --typefilter vzdump --limit 15 2>&1
fi

# ---------- TrueNAS ----------
if have midclt; then
  h "TRUENAS APPS AND VMS"
  midclt call app.query 2>/dev/null | python3 -c '
import json,sys
for a in json.load(sys.stdin): print(f"app  {a.get(\"name\")}: {a.get(\"state\")}")' 2>/dev/null || echo "(app.query unavailable)"
  midclt call vm.query 2>/dev/null | python3 -c '
import json,sys
for v in json.load(sys.stdin): print(f"vm   {v.get(\"name\")}: {(v.get(\"status\") or {}).get(\"state\")}")' 2>/dev/null || echo "(vm.query unavailable)"

  h "TRUENAS DATA PROTECTION"
  for task in pool.snapshottask replication cloudsync rsynctask; do
    echo "--- $task ---"
    midclt call "$task.query" 2>/dev/null | python3 -c '
import json,sys
keys=("dataset","name","description","source_datasets","target_dataset","path","transport",
      "remotehost","lifetime_value","lifetime_unit","schedule","enabled","state","job")
rows=json.load(sys.stdin)
if not rows: print("(none configured)")
for r in rows:
    print({k:r[k] for k in keys if k in r})' 2>/dev/null | redact || echo "(unavailable)"
  done
  echo "--- newest snapshot per dataset ---"
  zfs list -H -t snapshot -o name,creation -s creation 2>/dev/null \
    | awk -F'\t' '{split($1,a,"@"); last[a[1]]=$2} END {for (d in last) print d "  " last[d]}' | sort
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
