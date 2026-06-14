// Tauri IPC bridge
const { invoke } = window.__TAURI__.core;

// ── State ─────────────────────────────────────────────────────────────────────

const state = {
  // Wizard
  step: 0,
  agents: [],
  selectedAgent: null,
  folder: '',
  lifecycle: 'ephemeral',
  boxName: '',
  sync: 'mount',
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

function showView(id) {
  document.querySelectorAll('.view').forEach(v => v.classList.remove('active'));
  document.getElementById(id).classList.add('active');
}

// ── Home view ─────────────────────────────────────────────────────────────────

async function loadBoxes() {
  const list = document.getElementById('boxes-list');
  list.innerHTML = '<p class="hint">Loading…</p>';
  try {
    const boxes = await invoke('get_boxes');
    if (boxes.length === 0) {
      list.innerHTML = '<p class="hint">No persistent boxes yet. Create one with "+ New Box".</p>';
      return;
    }
    list.innerHTML = '';
    for (const b of boxes) {
      list.appendChild(makeBoxCard(b));
    }
  } catch (e) {
    list.innerHTML = `<p class="hint" style="color:var(--red)">${e}</p>`;
  }
}

function makeBoxCard(b) {
  const card = document.createElement('div');
  card.className = 'box-card';

  const dot = document.createElement('div');
  dot.className = `box-status-dot ${b.status}`;

  const info = document.createElement('div');
  info.className = 'box-card-info';
  info.innerHTML = `
    <div class="box-card-name">${esc(b.box_name)}</div>
    <div class="box-card-meta">${esc(b.agent_display_name)} · ${b.folder ? esc(b.folder) : 'no folder'}</div>
  `;

  const actions = document.createElement('div');
  actions.className = 'box-card-actions';

  const btnStop = btn('Stop', 'btn-ghost', async () => {
    try { await invoke('stop_box', { boxName: b.box_name }); loadBoxes(); }
    catch (e) { alert(e); }
  });
  const btnRemove = btn('Remove', 'btn-danger', async () => {
    if (!confirm(`Remove box "${b.box_name}"?`)) return;
    try { await invoke('remove_box', { boxName: b.box_name }); loadBoxes(); }
    catch (e) { alert(e); }
  });

  if (b.status === 'running') actions.appendChild(btnStop);
  actions.appendChild(btnRemove);

  card.append(dot, info, actions);
  return card;
}

// ── Wizard ────────────────────────────────────────────────────────────────────

const STEPS = ['agent', 'folder', 'lifecycle', 'provider', 'summary'];

async function startWizard() {
  state.step = 0;
  state.selectedAgent = null;
  state.folder = '';
  state.lifecycle = 'ephemeral';
  state.boxName = '';
  state.sync = 'mount';
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
      </div>`;
    document.getElementById('wiz-folder').oninput = e => { state.folder = e.target.value; };

  } else if (step === 'lifecycle') {
    title.textContent = 'Lifecycle & Sync';
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

  } else if (step === 'provider') {
    title.textContent = 'Provider';
    const isCompat = state.provider.type === 'openai-compatible';
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
        <label>Provider name</label>
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
      <div class="form-group">
        <label>Auth (secret reference)</label>
        <input id="wiz-pauth" type="text" placeholder="\${env:ANTHROPIC_API_KEY} or none" value="${esc(state.provider.auth)}" />
      </div>`;

    body.querySelector('#wiz-ptype').onchange = e => {
      state.provider.type = e.target.value;
      body.querySelector('#pbaseurl-group').style.display = e.target.value === 'openai-compatible' ? '' : 'none';
    };
    body.querySelector('#wiz-pname').oninput = e => { state.provider.name = e.target.value; };
    body.querySelector('#wiz-pmodel').oninput = e => { state.provider.model = e.target.value; };
    body.querySelector('#wiz-pbaseurl').oninput = e => { state.provider.base_url = e.target.value; };
    body.querySelector('#wiz-pauth').oninput = e => { state.provider.auth = e.target.value; };

  } else if (step === 'summary') {
    title.textContent = 'Summary';
    body.innerHTML = `
      <table class="summary-table">
        <tr><td>Agent</td><td>${esc(state.selectedAgent || '—')}</td></tr>
        <tr><td>Folder</td><td>${esc(state.folder)}</td></tr>
        <tr><td>Lifecycle</td><td>${esc(state.lifecycle)}</td></tr>
        ${state.lifecycle === 'persistent' ? `<tr><td>Box name</td><td>${esc(state.boxName)}</td></tr>` : ''}
        <tr><td>Sync</td><td>${esc(state.sync)}</td></tr>
        <tr><td>Provider type</td><td>${esc(state.provider.type)}</td></tr>
        <tr><td>Model</td><td>${esc(state.provider.model)}</td></tr>
        ${state.provider.base_url ? `<tr><td>Base URL</td><td>${esc(state.provider.base_url)}</td></tr>` : ''}
        <tr><td>Auth</td><td>${esc(state.provider.auth)}</td></tr>
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
    folder: state.folder.trim(),
    lifecycle: state.lifecycle,
    sync: state.sync,
    provider: {
      name: state.provider.name,
      type: state.provider.type,
      model: state.provider.model,
      base_url: state.provider.base_url || null,
      auth: state.provider.auth,
    },
  };

  setStatus('Preparing launch…');
  try {
    const info = await invoke('prepare_launch', { config });
    state.launchInfo = info;
    await invoke('open_in_terminal', { configPath: info.config_path });
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

document.getElementById('btn-new').onclick = startWizard;
document.getElementById('btn-refresh').onclick = loadBoxes;
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
