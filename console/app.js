// DevCoordinator2 Console: a static app over the edge's /api/<command> bridge.
// Every control calls the real API and re-reads state to prove the change.
'use strict';

const $ = (sel, root = document) => root.querySelector(sel);
const main = $('#main');
const state = { who: null };

function esc(value) {
  return String(value ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
function bytes(n) {
  if (n == null || Number.isNaN(Number(n))) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let v = Number(n); let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i += 1; }
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}
function pct(n) { return n == null ? '—' : `${Number(n).toFixed(1)}%`; }
function ago(iso) {
  if (!iso) return '—';
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  if (s < 90) return `${Math.round(s)}s ago`;
  if (s < 5400) return `${Math.round(s / 60)}m ago`;
  if (s < 172800) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}
function badge(text, kind) {
  const cls = kind || ({ running: 'ok', healthy: 'ok', passed: 'ok', stopped: '', degraded: 'warn', unhealthy: 'bad', failed: 'bad', 'timed-out': 'bad', cancelled: '', interrupted: 'warn', superseded: '', applying: 'warn', unknown: '', none: '' }[text] ?? '');
  return `<span class="badge ${cls}">${esc(text)}</span>`;
}
function spark(values, width = 90, height = 20) {
  if (!values || values.length < 2) return '<span class="muted">—</span>';
  const max = Math.max(...values, 1e-9); const step = width / (values.length - 1);
  const pts = values.map((v, i) => `${(i * step).toFixed(1)},${(height - (v / max) * (height - 2) - 1).toFixed(1)}`).join(' ');
  return `<svg class="spark" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" aria-label="trend"><polyline fill="none" stroke="#4c8dff" stroke-width="1.5" points="${pts}"/></svg>`;
}
function toast(text, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`; el.textContent = text;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), 6000);
}

class ApiError extends Error { constructor(code, message) { super(message); this.code = code; } }
async function api(command, args = {}) {
  let res;
  try {
    res = await fetch(`/api/${command}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(args) });
  } catch (error) {
    throw new ApiError('network', `edge unreachable: ${error.message}`);
  }
  if (res.status === 401) { location.href = `/auth/login?rt=${encodeURIComponent(location.pathname + location.hash)}`; throw new ApiError('unauthenticated', 'sign in'); }
  const body = await res.json().catch(() => ({ ok: false, error: { code: 'bad_response', message: `HTTP ${res.status}` } }));
  if (!body.ok) throw new ApiError(body.error?.code || 'error', body.error?.message || 'request failed');
  return body.result;
}

function setBanner(text) { const b = $('#banner'); b.hidden = !text; b.textContent = text || ''; }
function skeleton(rows = 4) { return Array.from({ length: rows }, () => '<div class="skeleton"></div>').join(''); }
function stateBlock(kind, text) {
  if (kind === 'denied') return `<div class="notice denied">Permission denied: ${esc(text)}</div>`;
  if (kind === 'error') return `<div class="notice"><strong>Could not load.</strong> ${esc(text)} <button class="btn btn-small" onclick="render()">Retry</button></div>`;
  return `<div class="notice muted">${esc(text)}</div>`;
}
function guard(fn) {
  return async (...args) => {
    try { return await fn(...args); } catch (error) {
      if (error.code === 'permission_denied') main.innerHTML = stateBlock('denied', error.message);
      else if (error.code !== 'unauthenticated') main.innerHTML = stateBlock('error', error.message);
      return null;
    }
  };
}
async function act(button, command, args, after) {
  button.disabled = true;
  try {
    const result = await api(command, args);
    toast(`${command}: ${result.state ?? result.status ?? 'done'}`, 'ok');
    if (after) await after(result);
  } catch (error) {
    toast(`${command} failed: ${error.message}`, 'bad');
  } finally { button.disabled = false; }
}
function bind(root) {
  root.querySelectorAll('[data-cmd]').forEach((btn) => {
    btn.addEventListener('click', () => {
      const args = JSON.parse(btn.dataset.args || '{}');
      if (btn.dataset.confirm && !window.confirm(btn.dataset.confirm)) return;
      if (btn.dataset.deleteData === 'ask') args.delete_data = window.confirm('Also delete persistent data (volumes, database)? Cancel keeps data.');
      act(btn, btn.dataset.cmd, args, () => render());
    });
  });
}

// --- Deployments ---------------------------------------------------------
const viewDeployments = guard(async () => {
  main.innerHTML = `<h1>Deployments</h1>${skeleton()}`;
  const { deployments, declared } = await api('deployment.list', {});
  if (!deployments.length) { main.innerHTML = `<h1>Deployments</h1>${stateBlock('empty', 'No deployments have been applied yet.')}`; return; }
  const admin = state.who?.administrator;
  main.innerHTML = `<h1>Deployments</h1><div class="tablewrap"><table><thead><tr><th>Deployment</th><th>State</th><th>Domain</th><th>Port</th><th>Generation</th><th>Updated</th><th>Actions</th></tr></thead><tbody>${deployments.map((d) => `<tr>
    <td class="wrap"><a href="#/deployments/${esc(d.deployment_id)}">${esc(d.name)}@${esc(d.source)}</a><div class="muted mono">${esc(d.deployment_id)}</div></td>
    <td>${badge(d.state)}</td><td class="wrap">${d.domain ? esc(d.domain) : '<span class="muted">—</span>'}</td><td>${d.current_generation ?? '—'}</td><td>${d.current_generation ?? '—'}</td><td>${ago(d.updated_at)}</td>
    <td class="actions">${['start', 'stop', 'restart'].map((a) => `<button class="btn btn-small" data-cmd="deployment.${a}" data-args='${esc(JSON.stringify({ deployment_id: d.deployment_id }))}'>${a}</button>`).join('')}
    ${admin ? `<button class="btn btn-small" data-cmd="deployment.apply" data-args='${esc(JSON.stringify({ deployment_id: d.deployment_id }))}'>apply</button>` : ''}</td></tr>`).join('')}</tbody></table></div>
    ${declared?.length ? `<h2>Declared, not applied</h2><ul>${declared.map((x) => `<li class="mono">${esc(x.name)}@${esc(x.source)}</li>`).join('')}</ul>` : ''}`;
  bind(main);
});

const viewDeployment = guard(async (id) => {
  main.innerHTML = `<h1>Deployment</h1>${skeleton()}`;
  const d = await api('deployment.status', { deployment_id: id });
  const admin = state.who?.administrator;
  const rows = d.components.map((c) => `<tr>
    <td class="wrap"><strong>${esc(c.name)}</strong><div class="muted">${esc(c.type)}${c.owned ? '' : ' · observed'}</div></td>
    <td>${badge(c.state)} ${badge(c.health)}</td><td>${c.generation ?? '—'}</td><td>${c.port ?? '—'}</td><td>${c.restarts ?? 0}</td>
    <td class="wrap mono">${esc(c.binding?.kind || '')} ${esc(c.binding?.identity || '')}</td>
    <td class="wrap">${c.last_error ? `<span class="badge bad">${esc(c.last_error)}</span>` : ''}</td>
    <td class="actions">${c.owned && c.independent_control ? ['start', 'stop', 'restart'].map((a) => `<button class="btn btn-small" data-cmd="deployment.${a}" data-args='${esc(JSON.stringify({ deployment_id: id, component: c.name }))}'>${a}</button>`).join('') : ''}
      ${c.owned ? `<button class="btn btn-small" data-logs="${esc(c.name)}">logs</button>` : ''}</td></tr>`).join('');
  main.innerHTML = `<h1>${esc(d.name)}@${esc(d.source)} ${badge(d.state)}</h1>
    <div class="grid"><div class="tile"><div class="k">Domain</div><div class="v">${d.domain ? esc(d.domain) : '—'}</div></div><div class="tile"><div class="k">Route port</div><div class="v">${d.route_port ?? '—'}</div></div><div class="tile"><div class="k">Generation</div><div class="v">${d.current_generation ?? '—'}${d.previous_generation ? ` <span class="muted">(prev ${d.previous_generation})</span>` : ''}</div></div><div class="tile"><div class="k">Expires</div><div class="v">${d.ttl_expires_at ? esc(d.ttl_expires_at) : 'never'}</div></div></div>
    <div class="actions" style="margin:12px 0">${['start', 'stop', 'restart'].map((a) => `<button class="btn" data-cmd="deployment.${a}" data-args='${esc(JSON.stringify({ deployment_id: id }))}'>${a}</button>`).join('')}
      ${admin ? `<button class="btn" data-cmd="deployment.apply" data-args='${esc(JSON.stringify({ deployment_id: id }))}'>apply</button><button class="btn" data-cmd="deployment.rollback" data-args='${esc(JSON.stringify({ deployment_id: id }))}'>rollback</button><button class="btn btn-danger" data-cmd="deployment.remove" data-args='${esc(JSON.stringify({ deployment_id: id }))}' data-confirm="Remove this deployment? Persistent data is kept unless you choose otherwise next." data-delete-data="ask">remove</button>` : ''}</div>
    <h2>Components</h2><div class="tablewrap"><table><thead><tr><th>Component</th><th>State</th><th>Gen</th><th>Port</th><th>Restarts</th><th>Binding</th><th>Error</th><th>Actions</th></tr></thead><tbody>${rows}</tbody></table></div>
    <div id="logs"></div><h2>Usage</h2><div id="usage">${skeleton(2)}</div>`;
  bind(main);
  main.querySelectorAll('[data-logs]').forEach((btn) => btn.addEventListener('click', async () => {
    btn.disabled = true;
    try { const r = await api('deployment.logs', { deployment_id: id, component: btn.dataset.logs, tail_lines: 200 }); $('#logs').innerHTML = `<h2>Logs: ${esc(btn.dataset.logs)}</h2><pre class="log">${esc(r.tail || '(empty)')}</pre>`; }
    catch (e) { toast(e.message, 'bad'); } finally { btn.disabled = false; }
  }));
  try {
    const usage = await Promise.all(d.components.filter((c) => c.owned).map(async (c) => {
      const hist = await api('health.history', { subject_kind: 'component', subject_id: `${id}/${c.name}`, metric: 'cpu_percent', minutes: 60 });
      const mem = await api('health.history', { subject_kind: 'component', subject_id: `${id}/${c.name}`, metric: 'memory_bytes', minutes: 60 });
      return `<tr><td>${esc(c.name)}</td><td>${hist.points.length ? pct(hist.points.at(-1).avg) : '—'} ${spark(hist.points.map((p) => p.avg))}</td><td>${mem.points.length ? bytes(mem.points.at(-1).avg) : '—'} ${spark(mem.points.map((p) => p.avg))}</td></tr>`;
    }));
    $('#usage').innerHTML = usage.length ? `<div class="tablewrap"><table><thead><tr><th>Component</th><th>CPU (last hour)</th><th>Memory (last hour)</th></tr></thead><tbody>${usage.join('')}</tbody></table></div>` : '<p class="muted">No owned components.</p>';
  } catch (e) { $('#usage').innerHTML = stateBlock(e.code === 'permission_denied' ? 'denied' : 'error', e.message); }
});

// --- Tests ---------------------------------------------------------------
const viewTests = guard(async () => {
  main.innerHTML = `<h1>Tests</h1>${skeleton()}`;
  const { runs } = await api('test.list', {});
  if (!runs.length) { main.innerHTML = `<h1>Tests</h1>${stateBlock('empty', 'No test runs yet. Start one from a repository with `devcoordinator2 test start`.')}`; return; }
  main.innerHTML = `<h1>Tests</h1><p class="muted">One current run per worktree. Logs load only on demand.</p><div class="tablewrap"><table><thead><tr><th>Repository / worktree</th><th>Test</th><th>Result</th><th>Duration</th><th>Started</th><th>Exit</th><th>Output</th><th>Actions</th></tr></thead><tbody>${runs.map((r) => `<tr>
    <td class="wrap"><strong>${esc(r.display_name)}</strong><div class="muted mono">${esc(r.worktree_path)}</div></td><td>${esc(r.test)}</td><td>${badge(r.status)}</td><td>${r.duration_seconds != null ? `${r.duration_seconds}s` : '—'}</td><td>${ago(r.started_at)}</td><td>${r.exit_code ?? '—'}</td>
    <td>${bytes(r.stdout_bytes_observed)}${r.stdout_truncated ? ' <span class="badge warn">truncated</span>' : ''} / ${bytes(r.stderr_bytes_observed)}</td>
    <td class="actions"><button class="btn btn-small" data-out="stdout" data-path="${esc(r.worktree_path)}">stdout</button><button class="btn btn-small" data-out="stderr" data-path="${esc(r.worktree_path)}">stderr</button>
      ${r.status === 'running' ? `<button class="btn btn-small" data-cmd="test.stop" data-args='${esc(JSON.stringify({ path: r.worktree_path }))}'>stop</button>` : `<button class="btn btn-small" data-cmd="test.start" data-args='${esc(JSON.stringify({ path: r.worktree_path }))}'>start</button>`}</td></tr>`).join('')}</tbody></table></div><div id="logs"></div>`;
  bind(main);
  main.querySelectorAll('[data-out]').forEach((btn) => btn.addEventListener('click', async () => {
    btn.disabled = true;
    try { const r = await api('test.output', { path: btn.dataset.path, stream: btn.dataset.out, tail_bytes: 16384 }); $('#logs').innerHTML = `<h2>${esc(btn.dataset.out)} tail ${r.truncated_before_tail ? '(earlier output omitted)' : ''}</h2><pre class="log">${esc(r.tail || '(empty)')}</pre><p class="muted mono">${esc(r.log_path)}</p>`; }
    catch (e) { toast(e.message, 'bad'); } finally { btn.disabled = false; }
  }));
});

// --- Health --------------------------------------------------------------
const viewHealth = guard(async (sub) => {
  if (sub === 'containers') return viewContainers();
  main.innerHTML = `<h1>Health</h1>${skeleton(6)}`;
  let summary = null; let denied = null;
  try { summary = await api('health.summary', {}); } catch (e) { if (e.code !== 'permission_denied') throw e; denied = e.message; }
  const repos = await api('health.repositories', {});
  const h = summary?.host || {};
  const tiles = summary ? `<div class="grid">
    <div class="tile"><div class="k">CPU</div><div class="v ${h.cpu_percent > 90 ? 'bad' : ''}">${pct(h.cpu_percent)}</div></div>
    <div class="tile"><div class="k">Memory used / available</div><div class="v">${bytes(h.memory_used)} <span class="muted">/ ${bytes(h.memory_available)}</span></div></div>
    <div class="tile"><div class="k">Filesystem used / free</div><div class="v ${h.fs_size && h.fs_free < h.fs_size * 0.1 ? 'bad' : ''}">${bytes(h.fs_used)} <span class="muted">/ ${bytes(h.fs_free)}</span></div></div>
    <div class="tile"><div class="k">Load 1/5/15 · swap</div><div class="v">${h.load_1 ?? '—'} / ${h.load_5 ?? '—'} / ${h.load_15 ?? '—'} <span class="muted">· ${bytes(h.swap_used)}</span></div></div>
    <div class="tile"><div class="k">Unhealthy deployments</div><div class="v ${summary.unhealthy_deployments.length ? 'bad' : ''}">${summary.unhealthy_deployments.length}</div></div>
    <div class="tile"><div class="k">Active tests</div><div class="v">${summary.active_tests.length}</div></div>
    <div class="tile"><div class="k">Containers by ownership</div><div class="v" style="font-size:13px">${Object.entries(summary.container_counts || {}).map(([k, v]) => `${esc(k)}: ${v}`).join('<br>') || '—'}</div></div>
    <div class="tile"><div class="k">Critical alerts</div><div class="v ${summary.alerts.some((a) => a.severity === 'critical') ? 'bad' : ''}">${summary.alerts.filter((a) => a.severity === 'critical').length}</div></div></div>
    ${summary.alerts.length ? `<h2>Current alerts</h2><ul>${summary.alerts.map((a) => `<li>${badge(a.severity, a.severity === 'critical' ? 'bad' : 'warn')} ${esc(a.message)} <span class="muted">since ${ago(a.opened_at)}</span></li>`).join('')}</ul>` : '<p class="muted">No active alerts.</p>'}
    <h2>Reconciliation</h2><p class="mono muted">managed ${pct(h.reconciliation?.managed_cpu_percent)} + DevCoordinator ${pct(h.reconciliation?.daemon_cpu_percent)} + other ${pct(h.reconciliation?.other_cpu_percent)} = host ${pct(h.cpu_percent)} · memory managed ${bytes(h.reconciliation?.managed_memory)} + daemon ${bytes(h.reconciliation?.daemon_memory)} + other ${bytes(h.reconciliation?.other_memory)}</p>` : stateBlock('denied', `${denied} (server-wide health is administrator-only)`);
  const rows = repos.repositories.map((r) => `<tr><td class="wrap"><strong>${esc(r.display_name)}</strong><div class="muted mono">${esc(r.root_path)}</div></td><td>${pct(r.cpu_percent)} ${spark(r.trend_cpu)}</td><td>${bytes(r.memory_bytes)} ${spark(r.trend_memory)}</td><td>${bytes(r.storage_bytes)}</td><td>${badge(r.health)}</td><td class="wrap">${r.deployments.map((d) => `<a href="#/deployments/${esc(d.deployment_id)}">${esc(d.name)}@${esc(d.source)}</a> ${badge(d.state)}`).join('<br>') || '<span class="muted">none</span>'}</td></tr>`).join('');
  main.innerHTML = `<h1>Health</h1><p><a href="#/health/containers">Containers view →</a></p>${tiles}<h2>Repositories</h2>${rows ? `<div class="tablewrap"><table><thead><tr><th>Repository</th><th>CPU</th><th>Memory</th><th>Storage</th><th>Health</th><th>Deployments</th></tr></thead><tbody>${rows}
    ${repos.devcoordinator ? `<tr><td><em>DevCoordinator</em></td><td>${pct(repos.devcoordinator.cpu_percent)}</td><td>${bytes(repos.devcoordinator.memory_bytes)}</td><td>${bytes(repos.devcoordinator.storage_bytes)}</td><td></td><td></td></tr><tr><td><em>Shared / unattributed</em></td><td>${pct(repos.shared_unattributed.cpu_percent)}</td><td>${bytes(repos.shared_unattributed.memory_bytes)}</td><td>${Object.entries(repos.shared_unattributed.storage || {}).map(([k, v]) => `${esc(k)} ${bytes(v)}`).join(', ') || '—'}</td><td></td><td></td></tr>` : ''}</tbody></table></div>` : stateBlock('empty', 'No repositories visible to you.')}`;
});

const viewContainers = guard(async () => {
  main.innerHTML = `<h1>Containers</h1>${skeleton(6)}`;
  const { containers, counts } = await api('health.containers', {});
  const admin = state.who?.administrator;
  main.innerHTML = `<h1>Containers</h1><p><a href="#/health">← Health</a> · ${Object.entries(counts).map(([k, v]) => `${esc(k)} ${v}`).join(' · ')}</p>${containers.length ? `<div class="tablewrap"><table><thead><tr><th>Name / identity</th><th>State</th><th>Class</th><th>Repository</th><th>Deployment / test</th><th>Caller</th><th>CPU</th><th>Memory</th><th>Layer</th><th>Created</th><th>TTL</th><th>Actions</th></tr></thead><tbody>${containers.map((c) => `<tr>
    <td class="wrap"><strong>${esc(c.name)}</strong><div class="muted mono">${esc(c.id)}</div><div class="muted">${esc(c.image)}</div></td><td>${badge(c.state, c.state === 'running' ? 'ok' : '')}</td><td>${badge(c.classification, c.classification === 'unmanaged' ? 'warn' : c.classification === 'orphaned-managed' ? 'bad' : 'ok')}</td>
    <td class="mono">${esc(c.repository_id || '—')}</td><td class="mono wrap">${esc(c.deployment_id ? `${c.deployment_id}/${c.component}` : c.run_id || '—')}</td><td>${c.caller_uid ?? '—'} ${esc(c.client || '')}</td><td>${pct(c.cpu_percent)}</td><td>${bytes(c.memory_bytes)}</td><td>${bytes(c.container_layer_bytes)}</td><td class="wrap">${esc(c.created)}</td><td>${c.ttl_seconds ?? '—'}</td>
    <td class="actions">${admin && (c.classification === 'orphaned-managed' || c.classification === 'managed-test') ? `<button class="btn btn-small btn-danger" data-cmd="health.container_remove" data-args='${esc(JSON.stringify({ container_id: c.id }))}' data-confirm="Remove this ${c.classification} container? Only DevCoordinator-owned ephemeral containers can be removed here.">remove</button>` : '<span class="muted">decide manually</span>'}</td></tr>`).join('')}</tbody></table></div>` : stateBlock('empty', 'No containers on this host.')}`;
  bind(main);
});

// --- Bugs ----------------------------------------------------------------
const viewBugs = guard(async () => {
  main.innerHTML = `<h1>Bugs</h1>${skeleton()}`;
  const { bugs } = await api('bug.list', {});
  main.innerHTML = `<h1>Open bugs</h1>${bugs.length ? `<div class="tablewrap"><table><thead><tr><th>Component</th><th>Summary</th><th>Expected / actual</th><th>Steps</th><th>Seen</th><th>Correlations</th><th></th></tr></thead><tbody>${bugs.map((b) => `<tr><td>${esc(b.component)}</td><td class="wrap"><strong>${esc(b.summary)}</strong><div class="muted mono">${esc(b.bug_id)} · ${esc(b.reporter)}</div></td><td class="wrap">${esc(b.expected)}<br><span class="muted">${esc(b.actual)}</span></td><td class="wrap">${esc(b.steps)}</td><td>${b.occurrences}× · ${ago(b.last_seen_at)}</td><td class="mono wrap">${esc(Object.entries(b.correlations || {}).map(([k, v]) => `${k}=${v}`).join(' ') || '—')}</td><td><button class="btn btn-small" data-cmd="bug.close" data-args='${esc(JSON.stringify({ bug_id: b.bug_id }))}'>close</button></td></tr>`).join('')}</tbody></table></div>` : stateBlock('empty', 'No open bugs.')}
    <h2>Report a bug</h2><form class="inline" id="bug-form">${['component', 'summary', 'expected', 'actual', 'steps'].map((f) => `<label class="f">${f}<${f === 'steps' ? 'textarea' : 'input'} name="${f}" required ${f === 'steps' ? '></textarea>' : '>'}</label>`).join('')}<button class="btn" type="submit">Report</button></form><p class="muted">Bounded atomic records only: no secrets, raw logs, or private paths.</p>`;
  bind(main);
  $('#bug-form').addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const btn = ev.target.querySelector('button');
    const args = Object.fromEntries(new FormData(ev.target).entries());
    await act(btn, 'bug.report', args, () => render());
  });
});

// --- Administration ------------------------------------------------------
const viewAdmin = guard(async () => {
  main.innerHTML = `<h1>Administration</h1>${skeleton(6)}`;
  const [users, deployments, telegram] = await Promise.all([api('user.list', {}), api('deployment.list', {}), api('telegram.list', {})]);
  const depOptions = deployments.deployments.map((d) => `<option value="${esc(d.deployment_id)}">${esc(d.name)}@${esc(d.source)}</option>`).join('');
  const roleOptions = users.roles.map((r) => `<option>${esc(r)}</option>`).join('');
  main.innerHTML = `<h1>Administration</h1>
    <h2>Users</h2>${users.users.length ? `<div class="tablewrap"><table><thead><tr><th>E-mail</th><th>Administrator</th><th>Grants</th><th>Last seen</th><th></th></tr></thead><tbody>${users.users.map((u) => `<tr><td class="wrap mono">${esc(u.email)}</td><td>${u.administrator ? badge('administrator', 'ok') : ''}</td><td class="wrap">${u.grants.map((g) => `<span class="badge">${esc(g.role)}</span> <span class="mono">${esc(g.deployment_id)}</span> <button class="btn btn-small" data-cmd="grant.remove" data-args='${esc(JSON.stringify({ email: u.email, deployment_id: g.deployment_id }))}'>×</button>`).join('<br>') || '<span class="muted">none</span>'}</td><td>${ago(u.last_seen_at)}</td><td><button class="btn btn-small btn-danger" data-cmd="user.remove" data-args='${esc(JSON.stringify({ email: u.email }))}' data-confirm="Remove ${esc(u.email)} and all grants? Takes effect on the next request.">remove</button></td></tr>`).join('')}</tbody></table></div>` : stateBlock('empty', 'No users.')}
    <form class="inline" id="grant-form"><label class="f">e-mail<input name="email" required></label><label class="f">deployment<select name="deployment_id" required>${depOptions}</select></label><label class="f">role<select name="role">${roleOptions}</select></label><button class="btn" type="submit">Set grant</button></form>
    <h2>Invitations</h2>${users.invitations.length ? `<div class="tablewrap"><table><thead><tr><th>E-mail</th><th>Administrator</th><th>Grants</th><th>Expires</th><th></th></tr></thead><tbody>${users.invitations.map((i) => `<tr><td class="mono wrap">${esc(i.email)}</td><td>${i.administrator ? 'yes' : ''}</td><td class="wrap mono">${esc(i.grants.map((g) => `${g.role}:${g.deployment_id}`).join(' ') || '—')}</td><td>${esc(i.expires_at)}</td><td><button class="btn btn-small" data-cmd="user.remove" data-args='${esc(JSON.stringify({ email: i.email }))}'>revoke</button></td></tr>`).join('')}</tbody></table></div>` : '<p class="muted">No outstanding invitations.</p>'}
    <form class="inline" id="invite-form"><label class="f">e-mail<input name="email" type="email" required></label><label class="f">deployment<select name="deployment_id"><option value="">(none)</option>${depOptions}</select></label><label class="f">role<select name="role">${roleOptions}</select></label><label class="f">administrator<input type="checkbox" name="administrator"></label><button class="btn" type="submit">Invite</button></form>
    <h2>Telegram</h2><p class="muted">Bot ${telegram.configured ? 'configured' : 'not configured'} · outbox pending ${telegram.outbox_pending} · last poll ${ago(telegram.last_poll_at)} ${telegram.last_error ? `· <span class="badge bad">${esc(telegram.last_error)}</span>` : ''}</p>
    ${telegram.chats.length ? `<div class="tablewrap"><table><thead><tr><th>Chat</th><th>E-mail</th><th>Subscriptions</th><th></th></tr></thead><tbody>${telegram.chats.map((c) => `<tr><td>${c.chat_id} <span class="muted">${esc(c.label || '')}</span></td><td class="mono wrap">${esc(c.email)}</td><td class="wrap">${c.subscriptions.map((s) => `<span class="badge">${esc(s)}</span> <button class="btn btn-small" data-cmd="telegram.unsubscribe" data-args='${esc(JSON.stringify({ chat_id: c.chat_id, scope: s }))}'>×</button>`).join(' ') || '<span class="muted">none</span>'}</td><td></td></tr>`).join('')}</tbody></table></div>` : '<p class="muted">No linked chats. Send /start to the bot to get a link code.</p>'}
    <form class="inline" id="link-form"><label class="f">link code<input name="code" required></label><label class="f">e-mail<input name="email" type="email" required></label><button class="btn" type="submit">Link chat</button></form>
    <form class="inline" id="sub-form"><label class="f">chat id<input name="chat_id" type="number" required></label><label class="f">scope<input name="scope" placeholder="server | deployment:&lt;id&gt; | repository:&lt;id&gt;" required></label><button class="btn" type="submit">Subscribe</button></form>
    <h2>Server</h2><div id="server" class="muted">${skeleton(1)}</div>`;
  bind(main);
  const submit = (form, cmd, build) => $(form).addEventListener('submit', async (ev) => { ev.preventDefault(); const fd = new FormData(ev.target); await act(ev.target.querySelector('button'), cmd, build(fd), () => render()); });
  submit('#grant-form', 'grant.set', (fd) => ({ email: fd.get('email'), deployment_id: fd.get('deployment_id'), role: fd.get('role') }));
  submit('#invite-form', 'user.invite', (fd) => ({ email: fd.get('email'), administrator: fd.get('administrator') === 'on', grants: fd.get('deployment_id') ? [{ deployment_id: fd.get('deployment_id'), role: fd.get('role') }] : [] }));
  submit('#link-form', 'telegram.link', (fd) => ({ code: fd.get('code'), email: fd.get('email') }));
  submit('#sub-form', 'telegram.subscribe', (fd) => ({ chat_id: Number(fd.get('chat_id')), scope: fd.get('scope') }));
  try {
    const [ping, edge] = await Promise.all([api('ping', {}), fetch('/healthz').then((r) => r.json())]);
    $('#server').innerHTML = `daemon ${esc(ping.daemon_version)} · schema ${ping.schema_version} · route document generation ${edge.route_generation} (${esc(edge.source)})`;
  } catch (e) { $('#server').textContent = e.message; }
});

// --- Router ----------------------------------------------------------------
async function render() {
  const hash = location.hash || '#/deployments';
  const [, view, arg] = hash.slice(1).split('/');
  document.querySelectorAll('#nav a').forEach((a) => a.classList.toggle('active', a.dataset.view === view));
  setBanner('');
  if (view === 'deployments') return arg ? viewDeployment(arg) : viewDeployments();
  if (view === 'tests') return viewTests();
  if (view === 'health') return viewHealth(arg);
  if (view === 'bugs') return viewBugs();
  if (view === 'admin') return viewAdmin();
  location.hash = '#/deployments';
  return undefined;
}
window.render = render;
window.addEventListener('hashchange', render);
(async () => {
  try {
    state.who = await api('user.whoami', {});
    $('#who-email').textContent = state.who.identity || 'local';
    $('#nav-admin').hidden = !state.who.administrator;
  } catch (e) { if (e.code !== 'unauthenticated') setBanner(`Cannot reach the coordinator: ${e.message}`); }
  render();
})();
