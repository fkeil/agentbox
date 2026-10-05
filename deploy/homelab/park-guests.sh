#!/usr/bin/env bash
# Park Proxmox guests: graceful shutdown with start-at-boot disabled, recorded
# in a ledger so every change can be undone. It never deletes anything.
#
#   park-guests.sh list              7-day average usage per guest
#   park-guests.sh park 101 105      shut down + disable onboot (asks first)
#   park-guests.sh unpark 101        restore original onboot + start
#   park-guests.sh status            what is parked, and its current state
#
# Run as root on the Proxmox host.

set -uo pipefail
LEDGER="${PARK_LEDGER:-/root/parked-guests.tsv}"
# Names that usually mean "other things depend on this".
RISKY='traefik|proxy|dns|pihole|pi-hole|adguard|unbound|opnsense|pfsense|router|firewall|wireguard|tailscale|vpn|homeassistant|home-assistant|hass|unifi|truenas|nas|pbs|backup'

die() { echo "error: $*" >&2; exit 1; }
command -v qm >/dev/null 2>&1 || die "run this on the Proxmox host"

tool_of()   { if qm status "$1" >/dev/null 2>&1; then echo qm; elif pct status "$1" >/dev/null 2>&1; then echo pct; else return 1; fi; }
label_of()  { [ "$1" = qm ] && echo vm || echo ct; }
name_of()   { "$1" config "$2" 2>/dev/null | awk -F': ' '/^(name|hostname):/ {print $2; exit}'; }
onboot_of() { local v; v=$("$1" config "$2" 2>/dev/null | awk -F': ' '/^onboot:/ {print $2; exit}'); echo "${v:-0}"; }
state_of()  { "$1" status "$2" 2>/dev/null | awk '{print $2}'; }
ha_managed() { command -v ha-manager >/dev/null 2>&1 && ha-manager status 2>/dev/null | grep -qE "(vm|ct):$1([^0-9]|$)"; }

cmd_list() {
  python3 - << 'PY'
import json, subprocess

def pvesh(path, *args):
    out = subprocess.run(["pvesh", "get", path, *args, "--output-format", "json"],
                         capture_output=True, text=True)
    return json.loads(out.stdout) if out.returncode == 0 and out.stdout.strip() else []

def avg(rows, key):
    vals = [r[key] for r in rows if r.get(key) is not None]
    return sum(vals) / len(vals) if vals else None

gib = lambda b: f"{(b or 0) / 2**30:.1f}"
print(f"{'ID':>5} {'KIND':4} {'NAME':24} {'STATUS':8} {'vCPU':>4} {'CPU%':>5} "
      f"{'MEM used/alloc GiB':>18} {'DISK GiB':>8} {'NET kB/s':>8}")
for g in sorted(pvesh("/cluster/resources", "--type", "vm"), key=lambda r: r["vmid"]):
    kind = "qemu" if g["type"] == "qemu" else "lxc"
    rrd = pvesh(f"/nodes/{g['node']}/{kind}/{g['vmid']}/rrddata",
                "--timeframe", "week", "--cf", "AVERAGE")
    cpu, mem = avg(rrd, "cpu"), avg(rrd, "mem")
    net = (avg(rrd, "netin") or 0) + (avg(rrd, "netout") or 0)
    status = "template" if g.get("template") else g.get("status", "?")
    print(f"{g['vmid']:>5} {'vm' if kind == 'qemu' else 'ct':4} {g.get('name', '')[:24]:24} "
          f"{status:8} {g.get('maxcpu', 0):>4} "
          f"{(f'{cpu * 100:.1f}' if cpu is not None else '-'):>5} "
          f"{(gib(mem) if mem is not None else '-'):>8} / {gib(g.get('maxmem')):>7} "
          f"{gib(g.get('maxdisk')):>8} {net / 1024:>8.1f}")
PY
  echo "Near-zero CPU and network for a week usually means nothing is using it."
}

cmd_park() {
  [ $# -gt 0 ] || die "give one or more VMIDs"
  local id t n
  for id in "$@"; do
    t=$(tool_of "$id") || die "no VM or CT with id $id"
    n=$(name_of "$t" "$id")
    printf '%6s %s  %-24s state=%-8s onboot=%s\n' "$id" "$(label_of "$t")" "$n" "$(state_of "$t" "$id")" "$(onboot_of "$t" "$id")"
    echo "$n" | grep -qiE "$RISKY" && echo "         ^ WARNING: looks like infrastructure. Stopping it may take other services down."
    ha_managed "$id" && echo "         ^ HA-managed: HA would restart it, so its HA state will be set to stopped."
  done
  read -r -p "Shut these down and disable start-at-boot? [y/N] " answer
  [[ "$answer" =~ ^[Yy]$ ]] || { echo "Nothing changed."; return 0; }

  for id in "$@"; do
    t=$(tool_of "$id"); n=$(name_of "$t" "$id")
    # Record the original onboot BEFORE changing it; a repeat park must not
    # overwrite the first entry, or unpark would restore onboot=0.
    if ! awk -F'\t' -v id="$id" '$1 == id {found=1} END {exit !found}' "$LEDGER" 2>/dev/null; then
      printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$t" "$n" "$(onboot_of "$t" "$id")" "$(date -Is)" >> "$LEDGER"
    fi
    "$t" set "$id" --onboot 0 >/dev/null
    if ha_managed "$id"; then
      ha-manager set "$(label_of "$t"):$id" --state stopped && echo "$id: HA state -> stopped"
      continue
    fi
    if [ "$(state_of "$t" "$id")" != running ]; then
      echo "$id: already $(state_of "$t" "$id"), onboot disabled"; continue
    fi
    echo "$id ($n): shutting down gracefully, up to 3 min..."
    if "$t" shutdown "$id" --timeout 180 >/dev/null 2>&1; then
      echo "$id: stopped"
    else
      # Deliberately not forced: a hard stop is pulling the power cord.
      echo "$id: did NOT stop in time and is still running. Usual cause: no guest agent or ACPI."
      echo "    Force it only if you accept an unclean shutdown:  $t stop $id"
    fi
  done
  echo "Ledger: $LEDGER   Undo with: $0 unpark <id>"
}

cmd_unpark() {
  [ $# -gt 0 ] || die "give one or more VMIDs"
  local id line t ob
  for id in "$@"; do
    line=$(awk -F'\t' -v id="$id" '$1 == id {print; exit}' "$LEDGER" 2>/dev/null)
    [ -n "$line" ] || { echo "$id: not in ledger, skipped"; continue; }
    t=$(cut -f2 <<< "$line"); ob=$(cut -f4 <<< "$line")
    "$t" set "$id" --onboot "$ob" >/dev/null
    if ha_managed "$id"; then ha-manager set "$(label_of "$t"):$id" --state started
    else "$t" start "$id" >/dev/null; fi
    awk -F'\t' -v id="$id" '$1 != id' "$LEDGER" > "$LEDGER.tmp" && mv "$LEDGER.tmp" "$LEDGER"
    echo "$id: onboot restored to $ob, started"
  done
}

cmd_status() {
  [ -s "$LEDGER" ] || { echo "Nothing parked."; return 0; }
  printf '%6s %-4s %-24s %-8s %-6s %s\n' ID KIND NAME NOW ONBOOT PARKED
  while IFS=$'\t' read -r id t n ob when; do
    printf '%6s %-4s %-24s %-8s %-6s %s\n' "$id" "$(label_of "$t")" "$n" "$(state_of "$t" "$id")" "was:$ob" "$when"
  done < "$LEDGER"
}

case "${1:-}" in
  list)   cmd_list ;;
  park)   shift; cmd_park "$@" ;;
  unpark) shift; cmd_unpark "$@" ;;
  status) cmd_status ;;
  *) sed -n '2,11p' "$0"; exit 1 ;;
esac
