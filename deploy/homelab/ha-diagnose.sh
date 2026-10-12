#!/usr/bin/env bash
# Read-only Home Assistant health check. Changes nothing.
#
#   HA_URL=http://<ha-ip>:8123 HA_TOKEN=<token> ./ha-diagnose.sh [root@<proxmox-ip>]
#
# Token: HA -> your profile -> Security -> Long-lived access tokens. Create one
# for this run and delete it afterwards. It is never printed.
# Given a Proxmox host, it also checks the HA guest itself, its USB
# passthrough, and whether park-guests.sh stopped anything it may depend on.
# If HA's guest isn't named like "homeassistant", set HA_VMID=<id>.

set -uo pipefail
: "${HA_URL:?set HA_URL, e.g. http://192.168.1.50:8123}"
: "${HA_TOKEN:?set HA_TOKEN to a long-lived access token}"
PVE="${1:-}"
SSH_CMD="${HA_SSH_CMD:-ssh -o BatchMode=yes -o ConnectTimeout=8}"

h()   { printf '\n========== %s ==========\n' "$1"; }
api() { curl -fsS --max-time 20 -H "Authorization: Bearer $HA_TOKEN" "$HA_URL$1"; }

h "REACHABILITY"
if out=$(api /api/ 2>&1); then
  echo "API: $out"
else
  echo "API FAILED: $out"
  echo "  401 = bad token. Connection refused/timeout = HA down or wrong URL."
  echo "  The Home Assistant checks below will fail too; the Proxmox section still runs."
fi

h "CORE STATE"
config=$(api /api/config 2>/dev/null)
[ -n "$config" ] && printf '%s' "$config" | python3 -I -c '
import json, sys
c = json.load(sys.stdin)
for k in ("version", "state", "safe_mode", "recovery_mode", "time_zone"):
    if k in c:
        print(f"{k:14} {c[k]}")
if c.get("state") not in (None, "RUNNING"):
    print("  <-- core is not RUNNING: still starting, or stuck during startup")
if c.get("safe_mode") or c.get("recovery_mode"):
    print("  <-- safe/recovery mode: configuration or integrations failed to load")
' || echo "(unavailable)"

h "ENTITIES"
states=$(api /api/states 2>/dev/null)
[ -n "$states" ] && printf '%s' "$states" | python3 -I -c '
import json, sys, collections
states = json.load(sys.stdin)
bad = [s for s in states if s["state"] == "unavailable"]
unknown = sum(1 for s in states if s["state"] == "unknown")
print(f"{len(states)} entities, {len(bad)} unavailable, {unknown} unknown")
by = collections.Counter(s["entity_id"].split(".")[0] for s in bad)
if by:
    print("unavailable by domain:", ", ".join(f"{d}={n}" for d, n in by.most_common()))
for s in sorted(bad, key=lambda s: s["entity_id"])[:50]:
    print("  ", s["entity_id"], "|", s.get("attributes", {}).get("friendly_name", ""))
print("A cluster of unavailable entities from one source (e.g. every Zigbee")
print("device) usually means one integration or radio is down, not many faults.")
' || echo "(unavailable)"

h "RECENT ERRORS (best effort)"
log=$(api /api/error_log 2>/dev/null)
if [ -n "$log" ]; then
  printf '%s\n' "$log" | python3 -I -c '
import re, sys, collections
pat = re.compile(r"^\S+ \S+ (ERROR|WARNING|CRITICAL) \([^)]*\) \[([^\]]+)\]")
count = collections.Counter()
for line in sys.stdin:
    m = pat.match(line)
    if m:
        count[(m.group(1), m.group(2))] += 1
print("most frequent sources:")
for (level, src), n in count.most_common(15):
    print(f"{n:6} {level:8} {src}")
'
  echo "--- last error lines ---"
  printf '%s\n' "$log" | grep -E ' (ERROR|CRITICAL) ' | tail -25 | sed -E 's/[A-Za-z0-9_-]{32,}/<REDACTED>/g'
else
  echo "Empty. Expected on HA OS 2025.11+, which no longer writes the log file"
  echo "this endpoint reads. The Proxmox section tries the journal instead."
fi

[ -n "$PVE" ] || { echo; echo "(No Proxmox host given; skipped guest, USB and parking checks.)"; exit 0; }

h "PROXMOX: $PVE"
$SSH_CMD "$PVE" "HA_VMID='${HA_VMID:-}' bash -s" << 'REMOTE'
echo "### host"; uptime; free -h 2>/dev/null | head -2

echo; echo "### parked with park-guests.sh (anything HA depends on?)"
if [ -s /root/parked-guests.tsv ]; then cat /root/parked-guests.tsv
else echo "(nothing parked)"; fi

echo; echo "### all guests"; qm list 2>/dev/null; pct list 2>/dev/null

check() {
  local t=$1 id=$2
  echo; echo "### HA guest: $t $id"
  $t status "$id"
  $t config "$id" 2>/dev/null | grep -vE 'password|sshkeys'
  echo "--- USB passthrough vs what the host can see"
  local any=0
  for dev in $($t config "$id" 2>/dev/null | awk -F'host=' '/^usb[0-9]+:/ {split($2, a, ","); print a[1]}'); do
    any=1
    if [[ $dev == *:* ]]; then
      lsusb 2>/dev/null | grep -qi "ID $dev" \
        && echo "$dev: present on host" \
        || echo "$dev: NOT PRESENT on host  <-- dongle unplugged, failed, or re-enumerated"
    else
      echo "$dev: passed by USB port path; breaks if the dongle moves port. Prefer vendor:product."
    fi
  done
  [ $any = 1 ] || echo "(no USB passthrough configured)"
  [ "$t" = qm ] || return 0
  echo "--- guest agent"
  if qm agent "$id" ping >/dev/null 2>&1; then echo "responding"
  else echo "NOT responding: VM hung, still booting, or agent disabled"; return 0; fi
  echo "--- guest disk usage"
  qm agent "$id" get-fsinfo 2>/dev/null | python3 -I -c '
import json, sys
seen = set()
for f in json.load(sys.stdin):
    total, used, mp = f.get("total-bytes"), f.get("used-bytes"), f.get("mountpoint")
    if not total or mp in seen:
        continue
    seen.add(mp)
    pct = 100 * used / total
    flag = "  <-- nearly full" if pct > 90 else ""
    print("%-34s %6.1f / %6.1f GiB  %5.1f%%%s" % (mp, used / 2**30, total / 2**30, pct, flag))
' || echo "(unavailable)"
  echo "--- core warnings from the journal (best effort, HA OS)"
  qm guest exec "$id" --timeout 25 -- journalctl CONTAINER_NAME=homeassistant -p warning -n 60 --no-pager 2>/dev/null \
    | python3 -I -c 'import json, sys; d = json.load(sys.stdin); print(d.get("out-data") or d.get("err-data") or "(empty)")' \
    || echo "(guest exec unavailable; read logs in HA: Settings -> System -> Logs)"
}

found=0
if [ -n "$HA_VMID" ]; then
  if qm status "$HA_VMID" >/dev/null 2>&1; then check qm "$HA_VMID"; found=1
  elif pct status "$HA_VMID" >/dev/null 2>&1; then check pct "$HA_VMID"; found=1; fi
else
  for id in $(qm list 2>/dev/null | awk 'NR>1 && tolower($2) ~ /home.?assistant|hass|haos/ {print $1}'); do check qm "$id"; found=1; done
  for id in $(pct list 2>/dev/null | awk 'NR>1 && tolower($NF) ~ /home.?assistant|hass|haos/ {print $1}'); do check pct "$id"; found=1; done
fi
[ $found = 1 ] || echo "No Home Assistant guest found by name. Re-run with HA_VMID=<id>."

echo; echo "### USB on host"
lsusb 2>/dev/null || echo "(no lsusb)"
ls -l /dev/serial/by-id/ 2>/dev/null || echo "(no serial adapters visible)"
echo "--- recent USB disconnects (kernel log)"
dmesg -T 2>/dev/null | grep -iE 'usb .*disconnect|cp210x|ch341|ftdi_sio|cdc_acm' | tail -15
REMOTE
