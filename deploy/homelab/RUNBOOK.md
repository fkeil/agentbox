# Runbook — for an agent executing this build

Written for Claude Code (or similar) running **on your own machines**, with
network access to the homelab. It executes the build in `README.md`, phase by
phase, stopping at a verification gate after each one.

Read `HANDOFF.md` for why the architecture is shaped this way. Do not redesign
it from this file alone.

## Starting

Run the agent from a clone of this repository, then:

```
Execute deploy/homelab/RUNBOOK.md. Start at Phase 0.
Stop at every GATE and report. Do not continue past a failed gate.
```

Run one phase per session if you can. The gates matter more than the speed.

---

## Rules for the agent

These override anything else in this repository.

**Never:**
- Commit a `.env`, a filled-in `bridge/config.json`, a bot token, or any
  password. Print secrets to the terminal only when the operator asks.
- Run `docker compose down -v`. The `-v` destroys the CouchDB volume, which is
  the vault.
- Run `ufw enable` without first allowing SSH and getting explicit operator
  confirmation. Locking the operator out of a headless box is the most likely
  way this build does real damage.
- Write iptables rules on TrueNAS. SCALE has no supported host firewall and
  hand-written rules do not survive updates.
- Change TrueNAS network settings, web UI ports, or pool/dataset layout.
- Delete or overwrite anything under the vault dataset.
- Continue past a failed GATE. Report and stop.

**Always:**
- State which host you are on before running anything.
- Show the command and its output for each verification.
- Treat a gate as failed if you cannot demonstrate it passing. Do not infer
  success from the absence of an error.

**Ask the operator before:** creating or destroying a VM, enabling a firewall,
restarting TrueNAS or Proxmox services, or anything that interrupts storage.

---

## Phase 0 — Survey · *runs anywhere with SSH to both hosts*

1. Run `homelab-survey.sh` on the Proxmox host and on TrueNAS `192.168.1.4`.
2. From the Traefik section of the Proxmox output, extract: the HTTPS
   entrypoint name, the ACME cert resolver name, the dynamic config directory,
   and the DNS zone used by existing `Host()` rules.
3. Substitute into the route files:

```bash
grep -rn '__[A-Z_]*__' traefik-routes/
sed -i 's/__TRAEFIK_ENTRYPOINT__/<name>/g; s/__TRAEFIK_CERTRESOLVER__/<name>/g; s/__DOMAIN__/<zone>/g' traefik-routes/*.yaml
```

Leave `__APPSTACK_VM_IP__` until Phase 2 creates the VM.

**GATE 0:** `grep -rn '__[A-Z_]*__' traefik-routes/` returns only
`__APPSTACK_VM_IP__`. Report the four values found.

---

## Phase 1 — GPU services · *runs on TrueNAS 192.168.1.4*

Precondition: `nvidia-smi` works on the TrueNAS host. If it does not, stop —
the operator installs drivers via Apps → Settings and reboots.

```bash
cd truenas-gpu && cp .env.example .env
# set APPS_PATH to the real pool
docker compose up -d
docker exec ollama ollama pull qwen2.5:14b
```

**GATE 1:** both must pass, from a *different* machine on the LAN:

```bash
curl -s http://192.168.1.4:11434/api/tags          # lists the model
curl -s -F file=@sample.wav -F model=Systran/faster-whisper-large-v3 \
     http://192.168.1.4:8000/v1/audio/transcriptions   # returns text
```

Use a real audio file. A 200 with empty text is a failure, not a pass.

---

## Phase 2 — App stack · *runs on the Proxmox host, then the new VM*

1. Create an Ubuntu VM with Docker. **Ask the operator first** — confirm
   storage, bridge, and resources. Record its IP.
2. Substitute `__APPSTACK_VM_IP__` in `traefik-routes/30-appstack.yaml`.
3. On the VM, mount the vault dataset:

```bash
mkdir -p /mnt/vault
echo '192.168.1.4:/mnt/tank/vault /mnt/vault nfs defaults,_netdev 0 0' >> /etc/fstab
mount -a && touch /mnt/vault/.writetest && rm /mnt/vault/.writetest
```

4. Deploy:

```bash
cd proxmox-appstack && cp .env.example .env
openssl rand -hex 32          # N8N_ENCRYPTION_KEY — record it for the operator
# set COUCHDB_PASSWORD; put the SAME value in bridge/config.json
chmod 600 .env bridge/config.json
docker compose up -d
curl -X PUT http://$USER:$PASS@localhost:5984/obsidian
curl -X PUT http://$USER:$PASS@localhost:5984/_users
```

**GATE 2:** all three containers `healthy`/`running`; `curl` on
`localhost:5984/obsidian` returns JSON; `/mnt/vault` is writable.

---

## Phase 3 — Prove the bridge · *runs on the app VM* — **the critical gate**

This validates the architecture. Do it before any client touches the vault and
before any real note exists.

```bash
mkdir -p /mnt/vault/_inbox
echo "# roundtrip test" > /mnt/vault/_inbox/roundtrip.md
sleep 15
curl -s http://$USER:$PASS@localhost:5984/obsidian/_all_docs | grep roundtrip
```

**GATE 3:** the document appears in CouchDB.

**If it fails, stop and escalate — do not work around it.** livesync-bridge is
community software and this is the piece coupling automation to the vault. The
fallback is a different sync design (paid Obsidian Sync with automation writing
through a desktop client), which is the operator's decision, not the agent's.
Check `docker logs livesync-bridge` and report what you find.

---

## Phase 4 — Traefik and remote access · *Proxmox host + operator's devices*

1. Copy `traefik-routes/*.yaml` into the dynamic directory from Phase 0.
   File-provider configs are hot-reloaded; do not restart Traefik.
2. Set up the Tailscale subnet router on the app VM:

```bash
tailscale up --advertise-routes=192.168.1.0/24 --accept-dns=false
```

   **Operator step:** approve the route in the Tailscale admin console. Without
   it, public DNS records pointing at `192.168.1.x` are unroutable from away.

**GATE 4a:** each route answers over HTTPS with a trusted certificate, from the
LAN. A cert error here usually means the wrong resolver name from Phase 0.

**GATE 4b:** with the operator's phone on cellular and Tailscale connected,
`https://couch.<domain>` responds. Test this *before* Phase 5.

**Operator steps (agent cannot do these):** install the LiveSync plugin on
desktop and iPhone, point both at `https://couch.<domain>` / database
`obsidian`, leave **E2EE off**. Populate from desktop first, then connect the
phone.

---

## Phase 5 — n8n workflow · *operator builds, agent assists*

Built in n8n's UI, so the agent advises rather than executes.

**Operator first:** create the bot with BotFather, get the token, and find your
own numeric Telegram user ID.

Build in this order, testing each before adding the next:

1. Telegram Trigger → filter on `message.from.id`. **Verify a message from
   another account is dropped before continuing.**
2. Typed notes → write `/vault/_inbox/<timestamp>-<message_id>.md`
3. Voice branch → `getFile`, download, POST to
   `http://192.168.1.4:8000/v1/audio/transcriptions`
4. Structuring → POST to `http://192.168.1.4:11434/api/generate`
5. Error workflow, retaining source audio until the `.md` is confirmed written

Then set the webhook secret:

```bash
curl -F "url=https://n8n.<domain>/webhook/<path>" \
     -F "secret_token=$(openssl rand -hex 16)" \
     https://api.telegram.org/bot<TOKEN>/setWebhook
```

and verify the `X-Telegram-Bot-Api-Secret-Token` header inside the workflow.

**GATE 5:** a voice note sent from the operator's phone becomes a `.md` in
`_inbox/` and appears in Obsidian on that same phone.

---

## Phase 6 — Hardening · *app VM + TrueNAS*

1. Firewall the app VM per `README.md`. **SSH rule first, operator confirms
   before `ufw enable`.** Do not firewall TrueNAS — see README.
2. ZFS snapshot task on the vault dataset.
3. Scheduled CouchDB dump to TrueNAS.
4. Back up `N8N_ENCRYPTION_KEY` somewhere separate from the n8n backup;
   without it that backup cannot be decrypted.

**GATE 6:** SSH still works from a new session after enabling the firewall —
verify before closing the current one. A snapshot and a dump both exist.

---

## Phase 7 — ComfyUI on the GB10 · *runs on the Dell, independent*

No dependency on any phase above. `sm_121` + aarch64 + CUDA 13: **Conda will
not work** — no wheels exist for that combination. Use pip against
`https://download.pytorch.org/whl/cu130`, or NVIDIA NGC containers, and prefer
an existing DGX Spark ComfyUI recipe over a clean install.

**GATE 7:** `torch.cuda.is_available()` is `True` and a sample Wan 2.2 render
completes. Renders go to a dataset outside the vault.

---

## If something breaks

- **403 from Traefik** — allowlist rejected the source. Tailscale needs
  `100.64.0.0/10`, already in `10-middlewares.yaml`.
- **404 from Traefik** — router did not load; check for leftover placeholders.
- **Cert error** — wrong resolver name from Phase 0.
- **Desktop vault syncs, phone does not** — CORS is missing
  `capacitor://localhost`.
- **Bridge silent** — confirm `passphrase` is empty and both peers share the
  same `group`.

Roll back a phase with `docker compose down` (never `-v`) and re-read the gate
before retrying.
