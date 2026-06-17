// Tauri IPC bridge
const { invoke } = window.__TAURI__.core;

// ── Theme ─────────────────────────────────────────────────────────────────────

function applyTheme(theme) {
  document.documentElement.setAttribute('data-theme', theme);
  document.getElementById('btn-theme').textContent = theme === 'light' ? '☾' : '☀';
  localStorage.setItem('agentbox-theme', theme);
}

(function initTheme() {
  applyTheme(localStorage.getItem('agentbox-theme') || 'dark');
})();

// ── State ─────────────────────────────────────────────────────────────────────

const state = {
  // Wizard
  step: 0,
  agents: [],
  selectedAgent: null,
  folder: '',
  projectName: '',
  lifecycle: 'ephemeral',
  boxName: '',
  sync: 'mount',
  egress: { preset: 'open', allow: '', deny: '' },
  piModelsJson: '',
  provider: {
    name: 'anthropic',
    type: 'anthropic',
    model: 'claude-sonnet-4-6',
    base_url: '',
    auth: 'none',
  },

  // Diff review
  diffs: [],
  selectedDiffIdx: 0,
  launchInfo: null,
  diffPollTimer: null,
};

// ── View routing ──────────────────────────────────────────────────────────────

let _autoRefreshTimer = null;

function showView(id) {
  document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
  document.getElementById(id).classList.add('active');
  // Auto-refresh boxes every 3 s on the home view; pause on other views.
  if (_autoRefreshTimer) { clearInterval(_autoRefreshTimer); _autoRefreshTimer = null; }
  if (id === 'view-home') {
    _autoRefreshTimer = setInterval(loadBoxes, 3000);
  }
}

// ── Home view ─────────────────────────────────────────────────────────────────

async function loadBoxes() {
  const list = document.getElementById('boxes-list');
  // Only show "Loading…" on first load (list is empty), not on auto-refresh ticks.
  if (!list.hasChildNodes()) {
    list.innerHTML = '<p class="hint">Loading…</p>';
  }
  try {
    const boxes = await invoke('get_boxes');
    if (boxes.length === 0) {
      list.innerHTML = '<p class="hint">No boxes yet. Create one with "+ New Box".</p>';
    } else {
      list.innerHTML = '';
      for (const b of boxes) {
        list.appendChild(makeBoxCard(b));
      }
    }
  } catch (e) {
    list.innerHTML = `<p class="hint" style="color:var(--red)">${e}</p>`;
  }
  loadImages();
}

function makeBoxCard(b) {
  const isEphemeral = b.lifecycle === 'ephemeral';
  const isOrphaned = isEphemeral && b.status !== 'running';
  const card = document.createElement('div');
  card.className = 'box-card' + (isOrphaned ? ' box-card-orphaned' : '');

  const dot = document.createElement('div');
  if (isOrphaned) {
    dot.className = 'box-status-dot orphaned';
    dot.title = 'Orphaned ephemeral container';
  } else {
    dot.className = `box-status-dot ${b.status}`;
  }

  const info = document.createElement('div');
  info.className = 'box-card-info';
  const agentLabel = b.project_name
    ? `${esc(b.agent_display_name)} - ${esc(b.project_name)}`
    : esc(b.agent_display_name);
  const lifecycleBadge = isOrphaned
    ? `<span class="lifecycle-badge ephemeral-badge">⚠ orphaned</span>`
    : `<span class="lifecycle-badge">${esc(b.lifecycle)}</span>`;
  info.innerHTML = `
    <div class="box-card-name">${esc(b.box_name)} ${lifecycleBadge}</div>
    <div class="box-card-meta">${agentLabel} · ${b.folder ? esc(b.folder) : 'no folder'}</div>
  `;

  const actions = document.createElement('div');
  actions.className = 'box-card-actions';

  if (isOrphaned) {
    // Stopped ephemeral container that didn't clean up: only Kill action
    actions.appendChild(btn('Kill', 'btn-danger', async () => {
      if (!confirm(`Force-remove container "${b.box_name}"?\n\nThis removes the container only. No state volume is affected.`)) return;
      try { await invoke('kill_box', { boxName: b.box_name }); loadBoxes(); }
      catch (e) { alert(e); }
    }));
  } else {
    // Persistent box: Stop / Attach / Remove
    const btnStop = btn('Stop', 'btn-ghost', async () => {
      try { await invoke('stop_box', { boxName: b.box_name }); loadBoxes(); }
      catch (e) { alert(e); }
    });
    const btnAttach = btn('Attach', 'btn-primary', async () => {
      const attachTitle = b.project_name
        ? `${b.agent_display_name} - ${b.project_name}`
        : b.agent_display_name;
      try { await invoke('attach_box_terminal', { boxName: b.box_name, title: attachTitle }); }
      catch (e) { alert(e); }
    });
    const btnRemove = btn('Remove', 'btn-danger', async () => {
      if (!confirm(`Remove box "${b.box_name}" and its state volume?`)) return;
      try { await invoke('remove_box', { boxName: b.box_name }); loadBoxes(); }
      catch (e) { alert(e); }
    });

    if (b.status === 'running') actions.appendChild(btnStop);
    if (b.status === 'stopped') actions.appendChild(btnAttach);
    actions.appendChild(btnRemove);
  }

  card.append(dot, info, actions);
  return card;
}

async function loadImages() {
  const list = document.getElementById('images-list');
  list.innerHTML = '<p class="hint">Loading…</p>';
  try {
    const images = await invoke('list_cache_images');
    if (images.length === 0) {
      list.innerHTML = '<p class="hint">No cache images. They are created on first agent launch.</p>';
      return;
    }
    list.innerHTML = '';
    for (const img of images) {
      list.appendChild(makeImageCard(img));
    }
  } catch (e) {
    list.innerHTML = `<p class="hint" style="color:var(--red)">${e}</p>`;
  }
}

async function pruneImages() {
  if (!confirm('Remove ALL cache images?\n\nAgents will be reinstalled on next launch.')) return;
  try {
    const count = await invoke('prune_cache_images');
    setStatus(`Pruned ${count} cache image(s).`);
    loadImages();
  } catch (e) {
    alert(`Failed to prune images: ${e}`);
  }
}

async function loadProfiles() {
  const list = document.getElementById('profiles-list');
  list.innerHTML = '<p class="hint">Loading…</p>';
  try {
    const profiles = await invoke('list_profiles_cmd');
    if (profiles.length === 0) {
      list.innerHTML = '<p class="hint">No profiles saved. Create one with: agentbox profile save &lt;name&gt; --from box.yaml</p>';
      return;
    }
    list.innerHTML = '';
    for (const p of profiles) {
      list.appendChild(makeProfileCard(p));
    }
  } catch (e) {
    list.innerHTML = `<p class="hint" style="color:var(--red)">${e}</p>`;
  }
}

function makeProfileCard(p) {
  const card = document.createElement('div');
  card.className = 'image-card';

  const info = document.createElement('div');
  info.className = 'image-card-info';
  info.innerHTML = `
    <div class="image-card-name">${esc(p.name)}</div>
    <div class="image-card-meta">${esc(p.agent)} · ${esc(p.provider_name)} / ${esc(p.model)}</div>
  `;

  const actions = document.createElement('div');
  actions.className = 'image-card-actions';
  actions.appendChild(btn('Run', 'btn-primary btn-sm', async () => {
    const folder = prompt(`Run profile "${p.name}"\n\nEnter workspace folder path:`);
    if (!folder || !folder.trim()) return;
    try {
      await invoke('run_profile_terminal', { name: p.name, folder: folder.trim(), title: `${p.agent} — ${p.name}` });
      setStatus(`Profile "${p.name}" started.`);
    } catch (e) { alert(e); }
  }));

  card.append(info, actions);
  return card;
}

async function loadManifests() {
  const list = document.getElementById('manifests-list');
  list.innerHTML = '<p class="hint">Loading…</p>';
  try {
    const entries = await invoke('list_manifests_cmd');
    if (entries.length === 0) {
      list.innerHTML = '<p class="hint">No manifests found. Add one with "+ Add".</p>';
      return;
    }
    list.innerHTML = '';
    for (const m of entries) {
      list.appendChild(makeManifestCard(m));
    }
  } catch (e) {
    list.innerHTML = `<p class="hint" style="color:var(--red)">${e}</p>`;
  }
}

function makeManifestCard(m) {
  const card = document.createElement('div');
  card.className = 'image-card';

  const info = document.createElement('div');
  info.className = 'image-card-info';
  const daemonBadge = m.is_daemon ? ' <span style="font-size:10px;padding:2px 5px;border-radius:3px;background:var(--bg3);color:var(--text-dim)">daemon</span>' : '';
  info.innerHTML = `
    <div class="image-card-name">${esc(m.display_name)}${daemonBadge}</div>
    <div class="image-card-meta">${esc(m.id)} · ${esc(m.source)}</div>
  `;

  const actions = document.createElement('div');
  actions.className = 'image-card-actions';
  if (m.source === 'user') {
    actions.appendChild(btn('Remove', 'btn-danger btn-sm', async () => {
      if (!confirm(`Remove user manifest "${m.id}"?`)) return;
      try {
        await invoke('remove_manifest_cmd', { id: m.id });
        setStatus(`Manifest "${m.id}" removed.`);
        loadManifests();
      } catch (e) { alert(e); }
    }));
  }

  card.append(info, actions);
  return card;
}

async function addManifest() {
  const source = prompt('Enter manifest URL (https://…) or local file path:');
  if (!source || !source.trim()) return;
  try {
    const id = await invoke('add_manifest_cmd', { source: source.trim() });
    setStatus(`Manifest "${id}" installed.`);
    loadManifests();
  } catch (e) {
    alert(`Failed to add manifest: ${e}`);
  }
}

function makeImageCard(img) {
  const card = document.createElement('div');
  card.className = 'image-card';

  const info = document.createElement('div');
  info.className = 'image-card-info';
  info.innerHTML = `
    <div class="image-card-name">${esc(img.agent_id)}</div>
    <div class="image-card-meta">${esc(img.image_name)} · ${img.size_mb.toFixed(1)} MB</div>
  `;

  const actions = document.createElement('div');
  actions.className = 'image-card-actions';
  actions.appendChild(btn('Delete', 'btn-danger btn-sm', async () => {
    if (!confirm(`Delete cache image for "${img.agent_id}"?\n\nThe agent will be reinstalled on next launch.`)) return;
    try { await invoke('remove_cache_image', { agentId: img.agent_id }); loadImages(); }
    catch (e) { alert(e); }
  }));

  card.append(info, actions);
  return card;
}

// ── Wizard ────────────────────────────────────────────────────────────────────

const STEPS = ['agent', 'folder', 'lifecycle', 'egress', 'provider', 'summary'];

async function startWizard() {
  state.step = 0;
  state.selectedAgent = null;
  state.folder = '';
  state.projectName = '';
  state.lifecycle = 'ephemeral';
  state.boxName = '';
  state.sync = 'mount';
  state.egress = { preset: 'open', allow: '', deny: '' };
  state.piModelsJson = '';
  state.provider = { name: 'anthropic', type: 'anthropic', model: 'claude-sonnet-4-6', base_url: '', auth: 'none' };

  try {
    state.agents = await invoke('get_agents');
  } catch (_) {
    state.agents = [];
  }

  showView('view-wizard');
  renderWizardStep();
}

function renderWizardStep() {
  const step = STEPS[state.step];
  const title = document.getElementById('wizard-step-title');
  const body = document.getElementById('wizard-body');
  const btnPrev = document.getElementById('btn-wizard-prev');
  const btnNext = document.getElementById('btn-wizard-next');

  btnPrev.style.display = state.step > 0 ? '' : 'none';
  btnNext.textContent = state.step === STEPS.length - 1 ? 'Launch →' : 'Next →';

  if (step === 'agent') {
    title.textContent = 'Choose Agent';
    body.innerHTML = '<div class="agent-grid"></div>';
    const grid = body.querySelector('.agent-grid');
    for (const a of state.agents) {
      const card = document.createElement('div');
      card.className = 'agent-card' + (state.selectedAgent === a.id ? ' selected' : '');
      card.innerHTML = `<div class="agent-card-name">${esc(a.display_name)}</div><div class="agent-card-source">${esc(a.source)}</div>`;
      card.onclick = () => {
        state.selectedAgent = a.id;
        grid.querySelectorAll('.agent-card').forEach(c => c.classList.remove('selected'));
        card.classList.add('selected');
      };
      grid.appendChild(card);
    }

  } else if (step === 'folder') {
    title.textContent = 'Select Folder';
    body.innerHTML = `
      <div class="form-group">
        <label>Host folder to mount</label>
        <input id="wiz-folder" type="text" placeholder="/home/you/myproject" value="${esc(state.folder)}" />
      </div>
      <div class="form-group">
        <label>Project name <span style="color:var(--text-dim);font-weight:normal">(optional — shown in window title and box list)</span></label>
        <input id="wiz-project-name" type="text" placeholder="MyProject" value="${esc(state.projectName)}" />
      </div>`;
    document.getElementById('wiz-folder').oninput = e => {
      state.folder = e.target.value;
      // Auto-fill project name from basename if not yet customised
      if (!state.projectName) {
        const basename = e.target.value.replace(/\\/g, '/').split('/').filter(Boolean).pop() || '';
        if (basename) document.getElementById('wiz-project-name').placeholder = basename;
      }
    };
    document.getElementById('wiz-project-name').oninput = e => { state.projectName = e.target.value; };

  } else if (step === 'lifecycle') {
    title.textContent = 'Lifecycle & Sync';
    // Auto-suggest box name from agent + folder basename
    if (!state.boxName && state.selectedAgent && state.folder) {
      const basename = state.folder.replace(/\\/g, '/').split('/').filter(Boolean).pop() || '';
      if (basename) state.boxName = `${state.selectedAgent}-${basename}`;
    }
    body.innerHTML = `
      <div class="form-group">
        <label>Lifecycle</label>
        <div class="radio-group">
          <label class="radio-opt ${state.lifecycle === 'ephemeral' ? 'selected' : ''}" id="opt-eph">
            <input type="radio" name="lc" value="ephemeral" ${state.lifecycle === 'ephemeral' ? 'checked' : ''} />
            <span class="opt-label">Ephemeral</span>
            <span class="opt-desc">Container removed after each session</span>
          </label>
          <label class="radio-opt ${state.lifecycle === 'persistent' ? 'selected' : ''}" id="opt-per">
            <input type="radio" name="lc" value="persistent" ${state.lifecycle === 'persistent' ? 'checked' : ''} />
            <span class="opt-label">Persistent</span>
            <span class="opt-desc">Named box survives across sessions</span>
          </label>
        </div>
      </div>
      <div class="form-group" id="box-name-group" style="${state.lifecycle === 'persistent' ? '' : 'display:none'}">
        <label>Box name</label>
        <input id="wiz-boxname" type="text" placeholder="my-project" value="${esc(state.boxName)}" />
      </div>
      <div class="form-group">
        <label>Sync mode</label>
        <div class="radio-group">
          <label class="radio-opt ${state.sync === 'mount' ? 'selected' : ''}" id="opt-mount">
            <input type="radio" name="sync" value="mount" ${state.sync === 'mount' ? 'checked' : ''} />
            <span class="opt-label">Mount</span>
            <span class="opt-desc">Live bind mount — changes immediate</span>
          </label>
          <label class="radio-opt ${state.sync === 'snapshot' ? 'selected' : ''}" id="opt-snap">
            <input type="radio" name="sync" value="snapshot" ${state.sync === 'snapshot' ? 'checked' : ''} />
            <span class="opt-label">Snapshot</span>
            <span class="opt-desc">Copy-in; review diff before writeback</span>
          </label>
        </div>
      </div>`;

    body.querySelectorAll('input[name=lc]').forEach(r => {
      r.onchange = () => {
        state.lifecycle = r.value;
        document.getElementById('opt-eph').classList.toggle('selected', state.lifecycle === 'ephemeral');
        document.getElementById('opt-per').classList.toggle('selected', state.lifecycle === 'persistent');
        document.getElementById('box-name-group').style.display = state.lifecycle === 'persistent' ? '' : 'none';
      };
    });
    body.querySelectorAll('input[name=sync]').forEach(r => {
      r.onchange = () => {
        state.sync = r.value;
        document.getElementById('opt-mount').classList.toggle('selected', state.sync === 'mount');
        document.getElementById('opt-snap').classList.toggle('selected', state.sync === 'snapshot');
      };
    });
    const bnInput = body.querySelector('#wiz-boxname');
    if (bnInput) bnInput.oninput = e => { state.boxName = e.target.value; };

  } else if (step === 'egress') {
    title.textContent = 'Network Egress';
    const isCustom = state.egress.preset === 'custom';
    const presets = [
      { value: 'open',          label: 'Open',          desc: 'Unrestricted internet access (default)' },
      { value: 'block-local',   label: 'Block local',   desc: 'Deny 10.x, 172.16.x, 192.168.x networks' },
      { value: 'provider-only', label: 'Provider only', desc: 'Only allow the AI provider + host gateway (deny all else)' },
      { value: 'custom',        label: 'Custom',        desc: 'Specify allow/deny rules manually' },
    ];
    body.innerHTML = `
      <div class="form-group">
        <label>Egress preset</label>
        <div class="radio-group">
          ${presets.map(p => `
          <label class="radio-opt ${state.egress.preset === p.value ? 'selected' : ''}" id="opt-eg-${p.value}">
            <input type="radio" name="egress-preset" value="${p.value}" ${state.egress.preset === p.value ? 'checked' : ''} />
            <span class="opt-label">${esc(p.label)}</span>
            <span class="opt-desc">${esc(p.desc)}</span>
          </label>`).join('')}
        </div>
      </div>
      <div id="egress-custom-group" style="${isCustom ? '' : 'display:none'}">
        <div class="form-group">
          <label>Deny rules <span style="color:var(--text-dim);font-weight:normal">(space or newline separated — IPs, CIDRs, hostnames, presets)</span></label>
          <textarea id="wiz-eg-deny" rows="3" style="width:100%;background:var(--bg3);border:1px solid var(--border);border-radius:var(--radius);color:var(--text);font-family:'SF Mono',Consolas,monospace;font-size:12px;padding:8px 10px;outline:none;resize:vertical" placeholder="local-network 192.0.2.0/24">${esc(state.egress.deny)}</textarea>
        </div>
        <div class="form-group">
          <label>Allow rules <span style="color:var(--text-dim);font-weight:normal">(evaluated after deny; deny wins)</span></label>
          <textarea id="wiz-eg-allow" rows="3" style="width:100%;background:var(--bg3);border:1px solid var(--border);border-radius:var(--radius);color:var(--text);font-family:'SF Mono',Consolas,monospace;font-size:12px;padding:8px 10px;outline:none;resize:vertical" placeholder="provider *.github.com">${esc(state.egress.allow)}</textarea>
        </div>
      </div>`;

    body.querySelectorAll('input[name=egress-preset]').forEach(r => {
      r.onchange = () => {
        state.egress.preset = r.value;
        presets.forEach(p => document.getElementById(`opt-eg-${p.value}`).classList.toggle('selected', state.egress.preset === p.value));
        document.getElementById('egress-custom-group').style.display = state.egress.preset === 'custom' ? '' : 'none';
      };
    });
    const denyEl = body.querySelector('#wiz-eg-deny');
    const allowEl = body.querySelector('#wiz-eg-allow');
    if (denyEl) denyEl.oninput = e => { state.egress.deny = e.target.value; };
    if (allowEl) allowEl.oninput = e => { state.egress.allow = e.target.value; };

  } else if (step === 'provider') {
    title.textContent = 'Provider';
    const isCompat = state.provider.type === 'openai-compatible';
    const isPi = state.selectedAgent === 'pi';
    const agentMeta = state.agents.find(a => a.id === state.selectedAgent) || {};
    const agentOauth = !!agentMeta.oauth_supported;
    // Detect if auth is currently set to oauth mode
    const isOauth = state.provider.auth === 'oauth';

    body.innerHTML = `
      <div class="form-group">
        <label>Provider type</label>
        <select id="wiz-ptype">
          <option value="anthropic" ${state.provider.type === 'anthropic' ? 'selected' : ''}>Anthropic</option>
          <option value="openai" ${state.provider.type === 'openai' ? 'selected' : ''}>OpenAI</option>
          <option value="openai-compatible" ${isCompat ? 'selected' : ''}>OpenAI-compatible (Ollama, etc.)</option>
        </select>
      </div>
      <div class="form-group">
        <label>Provider name${isPi ? ' <span style="font-weight:400;text-transform:none;letter-spacing:0">(must match key in models.json, e.g. "ollama")</span>' : ''}</label>
        <input id="wiz-pname" type="text" value="${esc(state.provider.name)}" />
      </div>
      <div class="form-group">
        <label>Model</label>
        <input id="wiz-pmodel" type="text" value="${esc(state.provider.model)}" />
      </div>
      <div class="form-group" id="pbaseurl-group" style="${isCompat ? '' : 'display:none'}">
        <label>Base URL</label>
        <input id="wiz-pbaseurl" type="text" placeholder="http://localhost:11434/v1" value="${esc(state.provider.base_url)}" />
      </div>

      ${agentOauth ? `
      <div class="form-group">
        <label>Auth method</label>
        <div class="radio-group">
          <label class="radio-opt ${!isOauth ? 'selected' : ''}" id="opt-auth-key">
            <input type="radio" name="auth-method" value="key" ${!isOauth ? 'checked' : ''} />
            <span class="opt-label">API Key</span>
            <span class="opt-desc">Inject a key from env / keychain / file</span>
          </label>
          <label class="radio-opt ${isOauth ? 'selected' : ''}" id="opt-auth-oauth">
            <input type="radio" name="auth-method" value="oauth" ${isOauth ? 'checked' : ''} />
            <span class="opt-label">OAuth / Subscription</span>
            <span class="opt-desc">Log in via browser — no API key needed</span>
          </label>
        </div>
      </div>
      <div id="auth-key-group" style="${isOauth ? 'display:none' : ''}">
        <div class="form-group">
          <label>Auth (secret reference)</label>
          <input id="wiz-pauth" type="text" placeholder="\${env:ANTHROPIC_API_KEY} or none" value="${esc(isOauth ? '' : state.provider.auth)}" />
        </div>
      </div>
      <div id="auth-oauth-info" style="${isOauth ? '' : 'display:none'}; background:var(--bg3); border:1px solid var(--border); border-radius:var(--radius); padding:14px 16px; margin-top:4px;">
        <div style="font-size:13px;font-weight:600;margin-bottom:8px;">How OAuth works in Agentbox</div>
        <ol style="font-size:12px;color:var(--text-dim);margin:0;padding-left:18px;line-height:1.8">
          <li>Click <strong>Launch →</strong> — a terminal window opens.</li>
          <li>The agent prints a URL like:<br><code style="font-size:11px;background:var(--bg2);padding:2px 6px;border-radius:3px">https://claude.ai/oauth/authorize?...</code></li>
          <li>Open that URL in your browser and log in with your account.</li>
          <li>The agent detects the completed login and starts.</li>
          <li>Your credentials are cached in a Docker volume — <strong>future runs skip the login</strong>.</li>
        </ol>
        <div style="font-size:11px;color:var(--text-dim);margin-top:10px;">
          Cache volume: <code>agentbox-oauth-${esc(state.selectedAgent || '')}</code>
        </div>
      </div>` : `
      <div class="form-group">
        <label>Auth (secret reference)</label>
        <input id="wiz-pauth" type="text" placeholder="\${env:ANTHROPIC_API_KEY} or none" value="${esc(state.provider.auth)}" />
      </div>`}

      ${isPi ? `
      <div class="form-group" id="pi-models-group">
        <label>Pi custom models.json <span style="font-weight:400;text-transform:none;letter-spacing:0">(optional — Ollama, vLLM, LM Studio, proxies)</span></label>
        <textarea id="wiz-pi-models" rows="8" style="width:100%;background:var(--bg3);border:1px solid var(--border);border-radius:var(--radius);color:var(--text);font-family:'SF Mono',Consolas,monospace;font-size:12px;padding:8px 10px;outline:none;resize:vertical" placeholder='{\n  "providers": {\n    "ollama": {\n      "baseUrl": "http://localhost:11434/v1",\n      "api": "openai-completions",\n      "apiKey": "ollama",\n      "models": [\n        { "id": "llama3.1:8b" },\n        { "id": "qwen2.5-coder:7b" }\n      ]\n    }\n  }\n}'>${esc(state.piModelsJson)}</textarea>
        <div id="pi-models-err" style="color:var(--red);font-size:12px;margin-top:4px;display:none"></div>
      </div>` : ''}`;

    body.querySelector('#wiz-ptype').onchange = e => {
      state.provider.type = e.target.value;
      body.querySelector('#pbaseurl-group').style.display = e.target.value === 'openai-compatible' ? '' : 'none';
    };
    body.querySelector('#wiz-pname').oninput = e => { state.provider.name = e.target.value; };
    body.querySelector('#wiz-pmodel').oninput = e => { state.provider.model = e.target.value; };
    body.querySelector('#wiz-pbaseurl') && (body.querySelector('#wiz-pbaseurl').oninput = e => { state.provider.base_url = e.target.value; });
    if (agentOauth) {
      body.querySelectorAll('input[name=auth-method]').forEach(r => {
        r.onchange = () => {
          const useOauth = r.value === 'oauth';
          state.provider.auth = useOauth ? 'oauth' : '';
          document.getElementById('opt-auth-key').classList.toggle('selected', !useOauth);
          document.getElementById('opt-auth-oauth').classList.toggle('selected', useOauth);
          document.getElementById('auth-key-group').style.display = useOauth ? 'none' : '';
          document.getElementById('auth-oauth-info').style.display = useOauth ? '' : 'none';
        };
      });
      const authInput = body.querySelector('#wiz-pauth');
      if (authInput) authInput.oninput = e => { state.provider.auth = e.target.value; };
    } else {
      body.querySelector('#wiz-pauth').oninput = e => { state.provider.auth = e.target.value; };
    }
    if (isPi) {
      body.querySelector('#wiz-pi-models').oninput = e => { state.piModelsJson = e.target.value; };
    }

  } else if (step === 'summary') {
    title.textContent = 'Summary';
    body.innerHTML = `
      <table class="summary-table">
        <tr><td>Agent</td><td>${esc(state.selectedAgent || '—')}</td></tr>
        <tr><td>Folder</td><td>${esc(state.folder)}</td></tr>
        ${state.projectName ? `<tr><td>Project name</td><td>${esc(state.projectName)}</td></tr>` : ''}
        <tr><td>Lifecycle</td><td>${esc(state.lifecycle)}</td></tr>
        ${state.lifecycle === 'persistent' ? `<tr><td>Box name</td><td>${esc(state.boxName)}</td></tr>` : ''}
        <tr><td>Sync</td><td>${esc(state.sync)}</td></tr>
        <tr><td>Egress</td><td>${esc(state.egress.preset)}</td></tr>
        <tr><td>Provider type</td><td>${esc(state.provider.type)}</td></tr>
        <tr><td>Model</td><td>${esc(state.provider.model)}</td></tr>
        ${state.provider.base_url ? `<tr><td>Base URL</td><td>${esc(state.provider.base_url)}</td></tr>` : ''}
        <tr><td>Auth</td><td>${state.provider.auth === 'oauth' ? 'OAuth / Subscription' : esc(state.provider.auth)}</td></tr>
      </table>
      <p style="margin-top:20px;color:var(--text-dim);font-size:12px;">
        Clicking "Launch" will open a terminal window running the agent.
        ${state.sync === 'snapshot' ? 'After the session ends, return here to review the diff.' : ''}
      </p>`;
  }
}

async function wizardNext() {
  const step = STEPS[state.step];

  // Validation per step
  if (step === 'agent' && !state.selectedAgent) {
    alert('Please select an agent.'); return;
  }
  if (step === 'folder' && !state.folder.trim()) {
    alert('Please enter a folder path.'); return;
  }
  if (step === 'provider' && state.selectedAgent === 'pi' && state.piModelsJson.trim()) {
    try { JSON.parse(state.piModelsJson); }
    catch (e) { alert('Pi models.json is not valid JSON:\n' + e.message + '\n\nThis field expects the Pi models.json format (JSON), not box.yaml YAML.'); return; }
  }
  if (step === 'lifecycle' && state.lifecycle === 'persistent' && !state.boxName.trim()) {
    alert('Please enter a box name for persistent lifecycle.'); return;
  }

  if (state.step === STEPS.length - 1) {
    await launchBox(); return;
  }
  state.step++;
  renderWizardStep();
}

function wizardPrev() {
  if (state.step > 0) { state.step--; renderWizardStep(); }
}

async function launchBox() {
  const config = {
    agent: state.selectedAgent,
    name: state.lifecycle === 'persistent' ? state.boxName : null,
    project_name: state.projectName.trim() || null,
    folder: state.folder.trim(),
    lifecycle: state.lifecycle,
    sync: state.sync,
    egress_preset: state.egress.preset,
    egress_allow: state.egress.preset === 'custom'
      ? state.egress.allow.trim().split(/[\s\n]+/).filter(Boolean)
      : [],
    egress_deny: state.egress.preset === 'custom'
      ? state.egress.deny.trim().split(/[\s\n]+/).filter(Boolean)
      : [],
    provider: {
      name: state.provider.name,
      type: state.provider.type,
      model: state.provider.model,
      base_url: state.provider.base_url || null,
      auth: state.provider.auth,
    },
    pi_models_json: (state.selectedAgent === 'pi' && state.piModelsJson.trim())
      ? state.piModelsJson : null,
  };

  setStatus('Preparing launch…');
  try {
    const info = await invoke('prepare_launch', { config });
    state.launchInfo = info;
    const agentLabel = (state.agents.find(a => a.id === state.selectedAgent) || {}).display_name || state.selectedAgent || '';
    const windowTitle = state.projectName.trim()
      ? `${agentLabel} - ${state.projectName.trim()}`
      : agentLabel;
    await invoke('open_in_terminal', { configPath: info.config_path, title: windowTitle || null });
    setStatus('Terminal opened. Waiting for session…');
    showView('view-home');
    loadBoxes();

    if (state.sync === 'snapshot') {
      setStatus('Snapshot mode — waiting for diff…');
      startDiffPoll(info.diff_path, info.config_path);
    } else {
      setStatus('');
    }
  } catch (e) {
    alert(`Launch failed: ${e}`);
    setStatus('');
  }
}

function startDiffPoll(diffPath, _cfgPath) {
  clearInterval(state.diffPollTimer);
  const folder = state.folder;
  state.diffPollTimer = setInterval(async () => {
    try {
      const diffs = await invoke('get_snapshot_diff', { hostFolder: folder });
      if (diffs && diffs.length > 0) {
        clearInterval(state.diffPollTimer);
        state.diffs = diffs;
        setStatus('');
        showDiffReview();
      }
    } catch (_) {}
  }, 2000);
}

// ── Diff review ───────────────────────────────────────────────────────────────

function showDiffReview() {
  const fileList = document.getElementById('diff-file-list');
  const viewer = document.getElementById('diff-viewer');
  fileList.innerHTML = '';
  viewer.innerHTML = '<p class="hint">Select a file to view its diff.</p>';

  for (let i = 0; i < state.diffs.length; i++) {
    const d = state.diffs[i];
    const item = document.createElement('div');
    item.className = 'diff-file-item';
    item.dataset.idx = i;

    const cb = document.createElement('input');
    cb.type = 'checkbox';
    cb.checked = true;
    cb.dataset.path = d.path;
    cb.onclick = e => e.stopPropagation();

    const badge = document.createElement('span');
    badge.className = `diff-badge badge-${d.kind}`;
    badge.textContent = d.kind[0].toUpperCase();

    const pathEl = document.createElement('span');
    pathEl.className = 'diff-file-path';
    pathEl.textContent = d.path;
    pathEl.title = d.path;

    item.append(cb, badge, pathEl);
    item.onclick = () => selectDiffFile(i);
    fileList.appendChild(item);
  }

  showView('view-diff');
  if (state.diffs.length > 0) selectDiffFile(0);
}

function selectDiffFile(idx) {
  document.querySelectorAll('.diff-file-item').forEach((el, i) => {
    el.classList.toggle('selected', i === idx);
  });
  state.selectedDiffIdx = idx;
  const d = state.diffs[idx];
  const viewer = document.getElementById('diff-viewer');

  if (!d.patch.trim()) {
    viewer.innerHTML = `<p class="hint">${d.kind === 'deleted' ? 'File deleted.' : '(no textual diff)'}</p>`;
    return;
  }

  const pre = document.createElement('pre');
  for (const raw of d.patch.split('\n')) {
    const span = document.createElement('span');
    span.className = 'diff-line';
    if (raw.startsWith('+++') || raw.startsWith('---')) {
      span.style.color = 'var(--text-dim)';
    } else if (raw.startsWith('+')) {
      span.className += ' diff-line-add';
    } else if (raw.startsWith('-')) {
      span.className += ' diff-line-del';
    } else if (raw.startsWith('@@')) {
      span.className += ' diff-line-hunk';
    }
    span.textContent = raw;
    pre.appendChild(span);
  }
  viewer.innerHTML = '';
  viewer.appendChild(pre);
}

async function applyApproved() {
  const approved = [];
  document.querySelectorAll('#diff-file-list input[type=checkbox]').forEach(cb => {
    if (cb.checked) approved.push(cb.dataset.path);
  });

  if (approved.length === 0) { alert('No files selected.'); return; }
  if (!confirm(`Apply ${approved.length} file(s) to ${state.folder}?`)) return;

  try {
    await invoke('apply_snapshot_changes', {
      hostFolder: state.folder,
      approvedPaths: approved,
    });
    setStatus('Changes applied.');
    showView('view-home');
    loadBoxes();
  } catch (e) {
    alert(`Failed to apply: ${e}`);
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function btn(label, cls, onClick) {
  const el = document.createElement('button');
  el.className = cls;
  el.textContent = label;
  el.onclick = onClick;
  return el;
}

function esc(s) {
  return String(s ?? '').replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;');
}

function setStatus(msg) {
  document.getElementById('topbar-status').textContent = msg;
}

// ── Event wiring ──────────────────────────────────────────────────────────────

document.getElementById('btn-theme').onclick = () => {
  const current = document.documentElement.getAttribute('data-theme');
  applyTheme(current === 'light' ? 'dark' : 'light');
};
document.getElementById('btn-new').onclick = startWizard;
document.getElementById('btn-refresh').onclick = loadBoxes;
document.getElementById('btn-refresh-images').onclick = loadImages;
document.getElementById('btn-prune-images').onclick = pruneImages;
document.getElementById('btn-refresh-profiles').onclick = loadProfiles;
document.getElementById('btn-refresh-manifests').onclick = loadManifests;
document.getElementById('btn-add-manifest').onclick = addManifest;
document.getElementById('btn-wizard-back').onclick = () => { showView('view-home'); loadBoxes(); };
document.getElementById('btn-wizard-next').onclick = wizardNext;
document.getElementById('btn-wizard-prev').onclick = wizardPrev;
document.getElementById('btn-diff-back').onclick = () => showView('view-home');
document.getElementById('btn-diff-apply').onclick = applyApproved;
document.getElementById('btn-diff-discard').onclick = () => {
  if (confirm('Discard all changes?')) { showView('view-home'); }
};

// ── Boot ──────────────────────────────────────────────────────────────────────

loadBoxes();
loadProfiles();
loadManifests();
_autoRefreshTimer = setInterval(loadBoxes, 3000);
