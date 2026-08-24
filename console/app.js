// DevCoordinator2 Console: a static app over the edge's /api/<command> bridge.
// Every control calls the real API and re-reads state to prove the change.
'use strict';

const $ = (sel, root = document) => root.querySelector(sel);
const main = $('#main');
const state = { who: null, healthRange: '24h', usageRange: '1h', decisionAspect: 'all', decisionLimit: 25, decisionBefore: null, decisionQuery: '', collapsed: new Set() };
const RANGES = {
  '1h': { minutes: 60, points: 60 },
  '24h': { minutes: 1440, points: 288 },
  '7d': { minutes: 10080, points: 336 },
  '30d': { minutes: 43200, points: 360 },
};

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
function meter(fraction) {
  if (fraction == null || Number.isNaN(fraction)) return '';
  const p = Math.max(0, Math.min(100, fraction * 100));
  const cls = fraction >= 0.9 ? 'bad' : fraction >= 0.75 ? 'warn' : '';
  return `<div class="meter"><span class="${cls}" style="width:${p.toFixed(1)}%"></span></div>`;
}
function minuteLabel(minute) {
  return minute ? `${minute.slice(5, 10)} ${minute.slice(11, 16)}` : '';
}
// Time-series chart: shaded min–max envelope plus the average line, with the
// scale and window bounds as plain HTML so nothing distorts or clips.
function chart(points, fmt, label) {
  if (!points || points.length < 2) {
    return `<div class="chartbox"><div class="chartmeta"><span>${esc(label)}</span></div><div class="notice muted">No history for this window yet. Samples accumulate while the coordinator runs.</div></div>`;
  }
  const w = 600; const h = 130; const T = 4; const B = 4;
  const ih = h - T - B;
  const maxV = Math.max(...points.map((p) => p.max ?? p.avg), 1e-9) * 1.05;
  const X = (i) => ((i / (points.length - 1)) * w);
  const Y = (v) => T + ih - (Math.min(v, maxV) / maxV) * ih;
  const upper = points.map((p, i) => [X(i), Y(p.max ?? p.avg)]);
  const lower = points.map((p, i) => [X(i), Y(p.min ?? p.avg)]);
  const band = [...upper, ...lower.reverse()].map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(' ');
  const avg = points.map((p, i) => `${X(i).toFixed(1)},${Y(p.avg).toFixed(1)}`).join(' ');
  const grid = [0.25, 0.5, 0.75].map((f) => `<line x1="0" x2="${w}" y1="${Y(maxV * f).toFixed(1)}" y2="${Y(maxV * f).toFixed(1)}" class="gridline"/>`).join('');
  const last = points.at(-1);
  return `<div class="chartbox">
    <div class="chartmeta"><span>${esc(label)}</span><span><span class="muted">scale 0–${fmt(maxV)} · now</span> <strong>${fmt(last.avg)}</strong></span></div>
    <svg class="chart" viewBox="0 0 ${w} ${h}" preserveAspectRatio="none" aria-label="${esc(label)} history">${grid}<polygon points="${band}" class="band"/><polyline points="${avg}" class="line" fill="none"/></svg>
    <div class="chartaxis"><span>${esc(minuteLabel(points[0].minute))}</span><span class="muted">min–max band, average line</span><span>${esc(minuteLabel(last.minute))}</span></div>
  </div>`;
}
function seg(options, current, dataKey, label = (o) => o) {
  return `<div class="seg" role="tablist">${options.map((o) => `<button type="button" class="${o === current ? 'active' : ''}" data-${dataKey}="${o}">${esc(label(o))}</button>`).join('')}</div>`;
}
function bindSeg(root, dataKey, apply) {
  const prop = dataKey.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  root.querySelectorAll(`[data-${dataKey}]`).forEach((btn) => btn.addEventListener('click', () => apply(btn.dataset[prop])));
}
function toast(text, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`; el.textContent = text;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), 6000);
}

class ApiError extends Error { constructor(code, message) { super(message); this.code = code; } }
let viewAbort = null; // render() aborts the previous view's pending reads so a slow stale load can never overwrite the current view
async function api(command, args = {}, abortable = true) {
  let res;
  try {
    res = await fetch(`/api/${command}`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(args), signal: abortable ? viewAbort?.signal : undefined });
  } catch (error) {
    if (error.name === 'AbortError') throw new ApiError('stale', 'superseded by navigation');
    throw new ApiError('network', `edge unreachable: ${error.message}`);
  }
  if (res.status === 401) { location.href = `/auth/login?rt=${encodeURIComponent(location.pathname + location.hash)}`; throw new ApiError('unauthenticated', 'sign in'); }
  const body = await res.json().catch(() => ({ ok: false, error: { code: 'bad_response', message: `HTTP ${res.status}` } }));
  if (!body.ok) throw new ApiError(body.error?.code || 'error', body.error?.message || 'request failed');
  return body.result;
}
async function history(kind, id, metric, rangeKey) {
  const r = RANGES[rangeKey] || RANGES['24h'];
  return api('health.history', { subject_kind: kind, subject_id: id, metric, minutes: r.minutes, points: r.points });
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
      if (error.code === 'stale') return null; // another view took over
      if (error.code === 'permission_denied') main.innerHTML = stateBlock('denied', error.message);
      else if (error.code !== 'unauthenticated') main.innerHTML = stateBlock('error', error.message);
      return null;
    }
  };
}
async function act(button, command, args, after) {
  button.disabled = true;
  try {
    const result = await api(command, args, false);
    toast(`${command}: ${result.state ?? result.status ?? result.domain ?? 'done'}`, 'ok');
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
function lifecycleButtons(id, component, cls = 'btn btn-small') {
  const args = component ? { deployment_id: id, component } : { deployment_id: id };
  return ['start', 'stop', 'restart'].map((a) => `<button class="${cls}" data-cmd="deployment.${a}" data-args='${esc(JSON.stringify(args))}'>${a}</button>`).join('');
}

// --- Deployments ---------------------------------------------------------
function deploymentRow(d, admin) {
  return `<tr>
    <td class="wrap"><a href="#/deployments/${esc(d.deployment_id)}">${esc(d.name)}@${esc(d.source)}</a><div class="muted mono">${esc(d.deployment_id)}</div></td>
    <td>${badge(d.state)} ${d.health && d.health !== 'unknown' && d.health !== d.state ? badge(d.health) : ''} ${d.observed_only ? badge('observed') : ''}</td>
    <td class="wrap">${d.domain ? esc(d.domain) : '<span class="muted">—</span>'}${admin ? ` <button class="btn btn-small" data-edit-domain="${esc(d.deployment_id)}" title="edit domain">✎</button>` : ''}</td>
    <td>${d.route_port ?? '—'}</td><td>${d.current_generation ?? '—'}</td><td>${ago(d.updated_at)}</td>
    <td class="actions">${lifecycleButtons(d.deployment_id)}
    ${!d.observed_only && admin ? `<button class="btn btn-small" data-cmd="deployment.apply" data-args='${esc(JSON.stringify({ deployment_id: d.deployment_id }))}'>apply</button>` : ''}</td></tr>`;
}
const viewDeployments = guard(async () => {
  main.innerHTML = `<h1>Deployments</h1>${skeleton()}`;
  const { deployments, declared } = await api('deployment.list', {});
  if (!deployments.length) { main.innerHTML = `<h1>Deployments</h1>${stateBlock('empty', 'No deployments have been applied yet.')}`; return; }
  const admin = state.who?.administrator;
  const groups = new Map();
  for (const d of deployments) {
    const key = d.repository_name || d.repository_id || 'unattributed';
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(d);
  }
  const body = [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([repo, ds]) =>
    `<tr class="grouphead"><td colspan="7">${esc(repo)} <span class="muted mono">${esc(ds[0].repository_id || '')}</span></td></tr>${ds.map((d) => deploymentRow(d, admin)).join('')}`).join('');
  main.innerHTML = `<h1>Deployments</h1><div class="tablewrap"><table><thead><tr><th>Deployment</th><th>State</th><th>Domain</th><th>Port</th><th>Generation</th><th>Updated</th><th>Actions</th></tr></thead><tbody>${body}</tbody></table></div>
    ${declared?.length ? `<h2>Declared, not applied</h2><ul>${declared.map((x) => `<li class="mono">${esc(x.name)}@${esc(x.source)}</li>`).join('')}</ul>` : ''}`;
  bind(main);
  bindDomainButtons(main, deployments);
});

// Pop-up domain editor, shared by the list rows (✎) and the detail page.
async function openDomainDialog(d) {
  document.getElementById('domain-dialog')?.remove();
  const needsTarget = d.observed_only && !d.route_port;
  let componentOptions = '';
  if (needsTarget) {
    let components = d.components;
    if (!components) {
      try { components = (await api('deployment.status', { deployment_id: d.deployment_id })).components; }
      catch (e) { toast(e.message, 'bad'); components = []; }
    }
    componentOptions = (components || []).map((c) => `<option>${esc(c.name)}</option>`).join('');
  }
  const dlg = document.createElement('dialog');
  dlg.id = 'domain-dialog';
  dlg.innerHTML = `<h2>Domain: ${esc(d.name)}@${esc(d.source)}</h2>
    ${d.repository_name ? `<p class="muted">${esc(d.repository_name)}</p>` : ''}
    <form id="domain-form" class="inline">
      <label class="f">domain label<input name="domain" value="${esc(d.domain || '')}" placeholder="my-app" pattern="[a-z0-9]([a-z0-9-]*[a-z0-9])?" title="lowercase DNS label" autofocus></label>
      ${needsTarget ? `<label class="f">host port<input name="port" type="number" min="1" max="65535" required></label><label class="f">component<select name="component">${componentOptions}</select></label>` : ''}
      <label class="f">public (no sign-in)<input type="checkbox" name="public" ${d.public ? 'checked' : ''}></label>
      <div class="actions" style="flex-basis:100%">
        <button class="btn" type="submit">Save domain</button>
        ${d.domain ? '<button class="btn" type="button" id="domain-clear">Remove domain</button>' : ''}
        <button class="btn" type="button" id="domain-cancel">Cancel</button>
      </div>
    </form>`;
  document.body.appendChild(dlg);
  dlg.addEventListener('close', () => dlg.remove());
  $('#domain-cancel', dlg).addEventListener('click', () => dlg.close());
  $('#domain-form', dlg).addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const fd = new FormData(ev.target);
    const args = { deployment_id: d.deployment_id, domain: fd.get('domain') || null, public: fd.get('public') === 'on' };
    if (fd.get('port')) { args.port = Number(fd.get('port')); if (fd.get('component')) args.component = fd.get('component'); }
    await act(ev.target.querySelector('button[type=submit]'), 'deployment.set_domain', args, () => { dlg.close(); return render(); });
  });
  $('#domain-clear', dlg)?.addEventListener('click', async (ev) => {
    if (!window.confirm('Remove the routed domain? The service stays up; only the edge route is removed.')) return;
    await act(ev.target, 'deployment.set_domain', { deployment_id: d.deployment_id, domain: null }, () => { dlg.close(); return render(); });
  });
  dlg.showModal();
}
function bindDomainButtons(root, deployments) {
  root.querySelectorAll('[data-edit-domain]').forEach((btn) => btn.addEventListener('click', () => {
    const d = deployments.find((x) => x.deployment_id === btn.dataset.editDomain);
    if (d) openDomainDialog(d);
  }));
}

const viewDeployment = guard(async (id) => {
  main.innerHTML = `<h1>Deployment</h1>${skeleton()}`;
  const d = await api('deployment.status', { deployment_id: id });
  const admin = state.who?.administrator;
  const obs = !!d.observed_only;
  const controllable = (c) => (obs ? c.binding?.kind === 'observed-container' : (c.owned && c.independent_control));
  const rows = d.components.map((c) => `<tr>
    <td class="wrap"><strong>${esc(c.name)}</strong>${c.display_name ? `<div class="muted">${esc(c.display_name)}</div>` : ''}<div class="muted">${esc(c.type)}${obs ? ' · exact recorded container' : (c.owned ? '' : ' · external')}</div></td>
    <td>${badge(c.state)} ${badge(c.health)}</td><td>${c.generation ?? '—'}</td><td>${c.port ?? '—'}</td><td>${c.restarts ?? '—'}</td>
    <td class="wrap mono">${esc(c.binding?.kind || '')} ${esc((c.binding?.identity || '').slice(0, 24))}</td>
    <td class="wrap">${c.last_error ? `<span class="badge bad">${esc(c.last_error)}</span>` : ''}</td>
    <td class="actions">${controllable(c) ? lifecycleButtons(id, c.name) : ''}
      ${c.owned || obs ? `<button class="btn btn-small" data-logs="${esc(c.name)}">logs</button>` : ''}</td></tr>`).join('');
  main.innerHTML = `<h1>${esc(d.name)}@${esc(d.source)} ${badge(d.state)} ${d.health && d.health !== d.state ? badge(d.health) : ''}</h1>
    <p class="muted">${d.repository_name ? `Repository: <strong>${esc(d.repository_name)}</strong> ` : ''}<span class="mono">${esc(d.repository_id || '')}</span></p>
    <div class="grid"><div class="tile"><div class="k">Domain ${admin ? '<button class="btn btn-small" id="edit-domain">edit</button>' : ''}</div><div class="v">${d.domain ? esc(d.domain) : '—'}</div>${d.public ? '<div class="muted">public (no sign-in)</div>' : ''}</div><div class="tile"><div class="k">Route port</div><div class="v">${d.route_port ?? '—'}</div></div><div class="tile"><div class="k">Generation</div><div class="v">${d.current_generation ?? '—'}${d.previous_generation ? ` <span class="muted">(prev ${d.previous_generation})</span>` : ''}</div></div><div class="tile"><div class="k">Expires</div><div class="v">${d.ttl_expires_at ? esc(d.ttl_expires_at) : 'never'}</div></div></div>
    ${obs ? '<p class="notice muted">Imported from the live host. Start, stop, restart, and logs act on the exact recorded containers. Configuration changes (apply, rollback, remove) require adopting the stack through repository configuration.</p>' : ''}
    <div class="actions" style="margin:12px 0">${lifecycleButtons(id, null, 'btn')}
      ${!obs && admin ? `<button class="btn" data-cmd="deployment.apply" data-args='${esc(JSON.stringify({ deployment_id: id }))}'>apply</button><button class="btn" data-cmd="deployment.rollback" data-args='${esc(JSON.stringify({ deployment_id: id }))}'>rollback</button><button class="btn btn-danger" data-cmd="deployment.remove" data-args='${esc(JSON.stringify({ deployment_id: id }))}' data-confirm="Remove this deployment? Persistent data is kept unless you choose otherwise next." data-delete-data="ask">remove</button>` : ''}</div>
    <h2>Components</h2><div class="tablewrap"><table><thead><tr><th>Component</th><th>State</th><th>Gen</th><th>Port</th><th>Restarts</th><th>Binding</th><th>Error</th><th>Actions</th></tr></thead><tbody>${rows}</tbody></table></div>
    <div id="logs"></div><h2>Usage ${seg(Object.keys(RANGES), state.usageRange, 'usage-range')}</h2><div id="usage">${skeleton(2)}</div>`;
  bind(main);
  $('#edit-domain')?.addEventListener('click', () => openDomainDialog(d));
  bindSeg(main, 'usage-range', (r) => { state.usageRange = r; render(); });
  main.querySelectorAll('[data-logs]').forEach((btn) => btn.addEventListener('click', async () => {
    btn.disabled = true;
    try { const r = await api('deployment.logs', { deployment_id: id, component: btn.dataset.logs, tail_lines: 200 }, false); $('#logs').innerHTML = `<h2>Logs: ${esc(btn.dataset.logs)}</h2><pre class="log">${esc(r.tail || '(empty)')}</pre>${r.log_path ? `<p class="muted mono">${esc(r.log_path)}</p>` : ''}`; }
    catch (e) { toast(e.message, 'bad'); } finally { btn.disabled = false; }
  }));
  try {
    const subjects = d.components
      .map((c) => (c.binding?.kind === 'observed-container'
        ? { name: c.name, kind: 'container', sid: c.binding.identity }
        : (c.owned ? { name: c.name, kind: 'component', sid: `${id}/${c.name}` } : null)))
      .filter(Boolean);
    const usage = await Promise.all(subjects.map(async (s) => {
      const [cpu, mem] = await Promise.all([
        history(s.kind, s.sid, 'cpu_percent', state.usageRange),
        history(s.kind, s.sid, 'memory_bytes', state.usageRange)]);
      return `<div class="chartpair"><h3>${esc(s.name)}</h3>${chart(cpu.points, pct, 'CPU')}${chart(mem.points, bytes, 'Memory')}</div>`;
    }));
    $('#usage').innerHTML = usage.length ? usage.join('') : '<p class="muted">No measured components.</p>';
  } catch (e) {
    if (e.code === 'stale') return;
    const el = $('#usage');
    if (el) el.innerHTML = stateBlock(e.code === 'permission_denied' ? 'denied' : 'error', e.message);
  }
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
    try { const r = await api('test.output', { path: btn.dataset.path, stream: btn.dataset.out, tail_bytes: 16384 }, false); $('#logs').innerHTML = `<h2>${esc(btn.dataset.out)} tail ${r.truncated_before_tail ? '(earlier output omitted)' : ''}</h2><pre class="log">${esc(r.tail || '(empty)')}</pre><p class="muted mono">${esc(r.log_path)}</p>`; }
    catch (e) { toast(e.message, 'bad'); } finally { btn.disabled = false; }
  }));
});

// --- Health --------------------------------------------------------------
function unhealthySection(summary) {
  const list = summary.unhealthy_deployments || [];
  if (!list.length) return '<h2>Unhealthy deployments</h2><p class="muted">All deployments are healthy.</p>';
  return `<h2>Unhealthy deployments</h2><div class="cards">${list.map((d) => `<div class="card bad-edge">
    <div class="cardhead"><a href="#/deployments/${esc(d.deployment_id)}"><strong>${esc(d.name)}@${esc(d.source)}</strong></a> ${d.repository_name ? `<span class="muted">in ${esc(d.repository_name)}</span>` : ''} ${badge(d.state)} ${d.observed_only ? badge('observed') : ''}</div>
    ${(d.reasons || []).length ? `<ul class="reasons">${d.reasons.map((r) => `<li><span class="mono">${esc(r.component)}</span> is ${badge(r.state, 'bad')}${r.detail ? ` — <span class="muted">${esc(r.detail)}</span>` : ''}</li>`).join('')}</ul>` : '<p class="muted">No component-level detail recorded.</p>'}
    <div class="actions">${lifecycleButtons(d.deployment_id)}<a class="btn btn-small" href="#/deployments/${esc(d.deployment_id)}">details &amp; logs</a></div>
  </div>`).join('')}</div>`;
}
const viewHealth = guard(async (sub) => {
  if (sub === 'containers') return viewContainers();
  main.innerHTML = `<h1>Health</h1>${skeleton(6)}`;
  let summary = null; let denied = null;
  try { summary = await api('health.summary', {}); } catch (e) { if (e.code !== 'permission_denied') throw e; denied = e.message; }
  const repos = await api('health.repositories', {});
  const h = summary?.host || {};
  const memFrac = h.memory_total ? h.memory_used / h.memory_total : null;
  const fsFrac = h.fs_size ? h.fs_used / h.fs_size : null;
  const tiles = summary ? `<div class="grid">
    <div class="tile"><div class="k">CPU (${h.ncpu ?? '?'} cores)</div><div class="v ${h.cpu_percent > 90 ? 'bad' : ''}">${pct(h.cpu_percent)}</div>${meter((h.cpu_percent ?? 0) / 100)}</div>
    <div class="tile"><div class="k">Memory</div><div class="v">${bytes(h.memory_used)} <span class="muted">of ${bytes(h.memory_total)}</span></div>${meter(memFrac)}</div>
    <div class="tile"><div class="k">Storage (root filesystem)</div><div class="v ${fsFrac > 0.9 ? 'bad' : ''}">${bytes(h.fs_used)} <span class="muted">of ${bytes(h.fs_size)}</span></div>${meter(fsFrac)}</div>
    <div class="tile"><div class="k">Load 1/5/15 · swap</div><div class="v">${h.load_1 ?? '—'} / ${h.load_5 ?? '—'} / ${h.load_15 ?? '—'}</div><div class="muted">swap ${bytes(h.swap_used)}</div></div>
    <div class="tile"><div class="k">Unhealthy deployments</div><div class="v ${summary.unhealthy_deployments.length ? 'bad' : 'ok'}">${summary.unhealthy_deployments.length}</div></div>
    <div class="tile"><div class="k">Active tests</div><div class="v">${summary.active_tests.length}</div></div>
    <div class="tile"><div class="k">Containers</div><div class="v" style="font-size:13px">${Object.entries(summary.container_counts || {}).map(([k, v]) => `${esc(k)}: ${v}`).join('<br>') || '—'}</div></div>
    <div class="tile"><div class="k">Critical alerts</div><div class="v ${summary.alerts.some((a) => a.severity === 'critical') ? 'bad' : 'ok'}">${summary.alerts.filter((a) => a.severity === 'critical').length}</div></div></div>
    ${unhealthySection(summary)}
    ${summary.alerts.length ? `<h2>Current alerts</h2><ul>${summary.alerts.map((a) => `<li>${badge(a.severity, a.severity === 'critical' ? 'bad' : 'warn')} ${esc(a.message)} <span class="muted">since ${ago(a.opened_at)}</span></li>`).join('')}</ul>` : '<p class="muted">No active alerts.</p>'}
    <h2>History ${seg(['24h', '7d', '30d'], state.healthRange, 'health-range')}</h2><div id="host-history" class="chartrow">${skeleton(3)}</div>
    <h2>Reconciliation</h2><p class="mono muted">managed ${pct(h.reconciliation?.managed_cpu_percent)} + DevCoordinator ${pct(h.reconciliation?.daemon_cpu_percent)} + other ${pct(h.reconciliation?.other_cpu_percent)} = host ${pct(h.cpu_percent)} · memory managed ${bytes(h.reconciliation?.managed_memory)} + daemon ${bytes(h.reconciliation?.daemon_memory)} + other ${bytes(h.reconciliation?.other_memory)}</p>` : stateBlock('denied', `${denied} (server-wide health is administrator-only)`);
  const rows = repos.repositories.map((r) => `<tr><td class="wrap"><strong>${esc(r.display_name)}</strong><div class="muted mono">${esc(r.root_path)}</div></td><td>${pct(r.cpu_percent)} ${spark(r.trend_cpu)}</td><td>${bytes(r.memory_bytes)} ${spark(r.trend_memory)}</td><td>${bytes(r.storage_bytes)} ${spark(r.trend_storage)}</td><td>${badge(r.health)}</td><td class="wrap">${r.deployments.map((d) => `<a href="#/deployments/${esc(d.deployment_id)}">${esc(d.name)}@${esc(d.source)}</a> ${badge(d.state)}`).join('<br>') || '<span class="muted">none</span>'}</td></tr>`).join('');
  main.innerHTML = `<h1>Health</h1><p><a href="#/health/containers">Containers view →</a></p>${tiles}<h2>Repositories</h2>${rows ? `<div class="tablewrap"><table><thead><tr><th>Repository</th><th>CPU</th><th>Memory</th><th>Storage</th><th>Health</th><th>Deployments</th></tr></thead><tbody>${rows}
    ${repos.devcoordinator ? `<tr><td><em>DevCoordinator</em></td><td>${pct(repos.devcoordinator.cpu_percent)}</td><td>${bytes(repos.devcoordinator.memory_bytes)}</td><td>${bytes(repos.devcoordinator.storage_bytes)}</td><td></td><td></td></tr><tr><td><em>Shared / unattributed</em></td><td>${pct(repos.shared_unattributed.cpu_percent)}</td><td>${bytes(repos.shared_unattributed.memory_bytes)}</td><td>${Object.entries(repos.shared_unattributed.storage || {}).map(([k, v]) => `${esc(k)} ${bytes(v)}`).join(', ') || '—'}</td><td></td><td></td></tr>` : ''}</tbody></table></div>` : stateBlock('empty', 'No repositories visible to you.')}`;
  bind(main);
  bindSeg(main, 'health-range', (r) => { state.healthRange = r; render(); });
  if (summary) {
    try {
      const [cpu, mem, sto] = await Promise.all([
        history('host', 'host', 'cpu_percent', state.healthRange),
        history('host', 'host', 'memory_used', state.healthRange),
        history('host', 'host', 'storage_bytes', state.healthRange)]);
      $('#host-history').innerHTML = chart(cpu.points, pct, 'Host CPU') + chart(mem.points, bytes, 'Host memory used') + chart(sto.points, bytes, 'Storage used');
    } catch (e) { const el = $('#host-history'); if (el) el.innerHTML = stateBlock('error', e.message); }
  }
});

const viewContainers = guard(async () => {
  main.innerHTML = `<h1>Containers</h1>${skeleton(6)}`;
  const { containers, counts } = await api('health.containers', {});
  const admin = state.who?.administrator;
  main.innerHTML = `<h1>Containers</h1><p><a href="#/health">← Health</a> · ${Object.entries(counts).map(([k, v]) => `${esc(k)} ${v}`).join(' · ')}</p>${containers.length ? `<div class="tablewrap"><table><thead><tr><th>Name / identity</th><th>State</th><th>Class</th><th>Repository</th><th>Deployment / test</th><th>Caller</th><th>CPU</th><th>Memory</th><th>Layer</th><th>Created</th><th>TTL</th><th>Actions</th></tr></thead><tbody>${containers.map((c) => `<tr>
    <td class="wrap"><strong>${esc(c.name)}</strong><div class="muted mono">${esc(c.id)}</div><div class="muted">${esc(c.image)}</div></td><td>${badge(c.state, c.state === 'running' ? 'ok' : '')}</td><td>${badge(c.classification, c.classification === 'unmanaged' ? 'warn' : c.classification === 'orphaned-managed' ? 'bad' : 'ok')}</td>
    <td class="mono">${esc(c.repository_id || '—')}</td><td class="mono wrap">${esc(c.deployment_id ? `${c.deployment_id}/${c.component}` : c.run_id || '—')}</td><td>${c.caller_uid ?? '—'} ${esc(c.client || '')}</td><td>${pct(c.cpu_percent)}</td><td>${bytes(c.memory_bytes)}</td><td>${bytes(c.container_layer_bytes)}</td><td class="wrap">${esc(c.created)}</td><td>${c.ttl_seconds ?? '—'}</td>
    <td class="actions">${admin && (c.classification === 'orphaned-managed' || c.classification === 'managed-test') ? `<button class="btn btn-small btn-danger" data-cmd="health.container_remove" data-args='${esc(JSON.stringify({ container_id: c.id }))}' data-confirm="Remove this ${c.classification} container? Only DevCoordinator-owned ephemeral containers can be removed here.">remove</button>` : `<span class="muted">${c.classification === 'observed-current' ? 'controlled via its deployment' : 'decide manually'}</span>`}</td></tr>`).join('')}</tbody></table></div>` : stateBlock('empty', 'No containers on this host.')}`;
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

// --- Plan (completion ledger, releases, previews) ------------------------
function locN(n) { return Number(n).toLocaleString('en-US'); }
function loc(n) { return n == null ? '' : `~${locN(n)} lines`; }
const PLAN_WORDS = { planned: 'planned', in_progress: 'being built', done: 'done', dropped: 'dropped', requested: 'preview requested', delivered: 'delivered' };
const PLAN_BADGE = { done: 'ok', delivered: 'ok', in_progress: 'warn', requested: 'warn' };
function planBadge(status) { return badge(PLAN_WORDS[status] || status, PLAN_BADGE[status] ?? ''); }

const viewPlanPicker = guard(async (kind) => {
  const title = kind === 'decisions' ? 'Decisions' : 'Plan';
  main.innerHTML = `<h1>${title}</h1>${skeleton()}`;
  const { repositories } = await api('plan.overview', {});
  if (!repositories.length) { main.innerHTML = `<h1>${title}</h1>${stateBlock('empty', 'No repositories visible to you.')}`; return; }
  main.innerHTML = `<h1>${title}</h1><p class="muted">Pick a repository.</p><div class="tablewrap"><table><thead><tr><th>Repository</th><th>Now building</th><th>Progress</th><th>Open tasks</th><th></th></tr></thead><tbody>${repositories.map((r) => `<tr>
    <td class="wrap"><a href="#/${kind}/${esc(r.repository_id)}"><strong>${esc(r.display_name)}</strong></a></td>
    <td class="wrap">${r.current_release ? `${esc(r.current_release.name)} ${planBadge(r.current_release.status)}` : '<span class="muted">no releases planned yet</span>'}${r.preview_requested ? ` ${badge('preview requested', 'warn')}` : ''}</td>
    <td>${r.loc_total ? `${meter(r.loc_done / r.loc_total)}<span class="muted">done ${locN(r.loc_done)} of ${locN(r.loc_total)} lines</span>` : '<span class="muted">nothing sized yet</span>'}</td>
    <td>${r.open_tasks}</td>
    <td class="actions"><a class="btn btn-small" href="#/plan/${esc(r.repository_id)}">plan</a><a class="btn btn-small" href="#/decisions/${esc(r.repository_id)}">decisions</a></td></tr>`).join('')}</tbody></table></div>`;
});

// Layout is pure: leaf tasks advance a global lines-of-code cursor, parents
// span their descendants, releases group in sequence, backlog trails.
function computeGantt(releases, tasks, collapsed) {
  const known = new Set(releases.map((r) => r.release_id));
  const byId = new Map(tasks.map((t) => [t.task_id, t]));
  const groupOf = (t) => (t.release_id && known.has(t.release_id) ? t.release_id : null);
  const inTree = (t) => { const p = byId.get(t.parent_task_id); return !!p && groupOf(p) === groupOf(t); };
  const order = (a, b) => (a.position - b.position) || (a.seq - b.seq);
  const kids = new Map();
  for (const t of tasks) {
    if (!inTree(t)) continue;
    if (!kids.has(t.parent_task_id)) kids.set(t.parent_task_id, []);
    kids.get(t.parent_task_id).push(t);
  }
  kids.forEach((list) => list.sort(order));
  const rows = []; const groups = []; let cursor = 0;
  const walk = (t, depth, hidden) => {
    const children = kids.get(t.task_id) || [];
    const row = { task: t, depth, isParent: children.length > 0, hidden, start: cursor, width: 0, subtreeLoc: 0, collapsed: collapsed.has(t.task_id) };
    rows.push(row);
    if (!children.length) {
      row.width = t.estimated_loc || 0;
      row.subtreeLoc = row.width;
      cursor += row.width;
    } else {
      for (const c of children) row.subtreeLoc += walk(c, depth + 1, hidden || row.collapsed).subtreeLoc;
      row.width = cursor - row.start;
    }
    return row;
  };
  for (const release of [...releases, null]) {
    const gid = release ? release.release_id : null;
    const group = { release, start: cursor };
    const mark = rows.length;
    for (const t of tasks.filter((x) => groupOf(x) === gid && !inTree(x)).sort(order)) walk(t, 0, false);
    group.rows = rows.slice(mark);
    group.end = cursor;
    if (release || group.rows.length) groups.push(group);
  }
  return { groups, total: cursor };
}

const viewPlan = guard(async (repoId) => {
  main.innerHTML = `<h1>Plan</h1>${skeleton(6)}`;
  const model = await api('plan.overview', { repository_id: repoId });
  const admin = state.who?.administrator;
  const { groups, total } = computeGantt(model.releases, model.tasks, state.collapsed);
  const pctOf = (v) => `${((v / total) * 100).toFixed(2)}%`;
  const requested = model.preview_requested || [];
  const requestBlock = requested.length
    ? `<p class="notice muted">Preview requested ${ago(requested[0].requested_at)}. The agent will put the current work online; a link appears on the preview release when it is ready.</p>`
    : (admin ? `<p><button class="btn" data-cmd="release.request" data-args='${esc(JSON.stringify({ repository_id: repoId }))}' data-confirm="Ask the agent to put the current work online for you to try?">Request preview now</button></p>` : '');
  const releaseHead = (g) => {
    const r = g.release;
    const droppable = admin && (!r || r.status !== 'delivered');
    const span = r && total && g.end > g.start ? `<div class="grelspan" style="left:${pctOf(g.start)};width:${pctOf(g.end - g.start)}"></div>` : '';
    const where = r?.url && /^https:\/\//.test(r.url) ? ` <a href="${esc(r.url)}" target="_blank" rel="noopener">Open the app ↗</a>`
      : (r?.status === 'delivered' && r.port ? ` <span class="muted">runs on server port ${Number(r.port)}</span>` : '');
    const progress = r ? (r.loc_total ? `done ${locN(r.loc_done)} of ${locN(r.loc_total)} lines` : `${r.tasks_done} of ${r.tasks_total} tasks done`) : '';
    return `<div class="grow grel"${droppable ? ` data-drop-release="${r ? esc(r.release_id) : ''}"` : ''}>
      <div class="glabel"><strong>${r ? esc(r.name) : 'Not scheduled yet'}</strong>${r ? ` ${planBadge(r.status)}` : ''}${r && r.kind === 'preview' && r.status !== 'requested' ? ` ${badge('preview')}` : ''}${where}</div>
      <div class="gtrack">${span}<span class="grelmeta">${esc(progress)}</span></div></div>`;
  };
  const taskRow = (row) => {
    const t = row.task;
    const movable = admin && t.status !== 'done';
    const actions = admin ? `<span class="actions">${movable ? `<button class="btn btn-small" data-move-task="${esc(t.task_id)}">move</button>` : ''}${t.status === 'planned' || t.status === 'in_progress' ? `<button class="btn btn-small" data-cmd="task.update" data-args='${esc(JSON.stringify({ task_id: t.task_id, status: 'dropped' }))}' data-confirm='Drop "${esc(t.title)}"? The agent will not build it. You can ask for it again later.'>drop</button>` : ''}</span>` : '';
    const collapse = row.isParent ? `<button class="btn btn-small gcollapse" data-collapse="${esc(t.task_id)}" title="${row.collapsed ? 'show subtasks' : 'hide subtasks'}">${row.collapsed ? '▸' : '▾'}</button> ` : '';
    const bar = !total || !row.width ? ''
      : (row.isParent ? `<div class="gbar parent" style="left:${pctOf(row.start)};width:${pctOf(row.width)}"></div>`
        : `<div class="gbar ${esc(t.status)}" style="left:${pctOf(row.start)};width:${pctOf(row.width)}" title="${esc(t.title)} — ${esc(loc(t.estimated_loc) || 'not sized')}"><span class="gdone"></span></div>`);
    return `<div class="grow gtask${row.hidden ? ' ghidden' : ''}"${movable ? ` draggable="true" data-drag-task="${esc(t.task_id)}"` : ''} data-task-row="${esc(t.task_id)}">
      <div class="glabel" style="padding-left:${8 + row.depth * 14}px">${movable ? '<span class="ghandle" title="drag to move">⠿</span> ' : ''}${collapse}${esc(t.title)} <span class="muted">${esc(row.isParent ? loc(row.subtreeLoc) : (loc(t.estimated_loc) || ''))}</span> ${planBadge(t.status)}${t.kind === 'user_feedback' ? ` ${badge('your request')}` : ''}${actions}${t.impact ? `<div class="muted gimpact">${esc(t.impact)}</div>` : ''}</div>
      <div class="gtrack">${bar}</div></div>`;
  };
  const boundaries = total ? groups.slice(1).map((g) => g.start).filter((s) => s > 0 && s < total) : [];
  const gantt = `<div class="gantt">
    <div class="gbounds">${boundaries.map((b) => `<i style="left:${pctOf(b)}"></i>`).join('')}</div>
    <div class="grow gaxis"><div class="glabel muted">task</div><div class="gtrack"><span>0</span><span>${total ? esc(`${locN(total)} lines planned`) : 'nothing sized yet'}</span></div></div>
    ${groups.map((g) => releaseHead(g) + g.rows.map(taskRow).join('')).join('')}
  </div>`;
  const commentForm = admin ? `<h2>Ask for a change</h2><form class="inline" id="comment-form">
    <label class="f">what you want<input name="title" required maxlength="120" placeholder="I tested it — the export button gives an error"></label>
    <label class="f">why it matters (optional)<textarea name="impact"></textarea></label>
    <button class="btn" type="submit">Send to the agent</button></form>
    <p class="muted">Plain language is enough — the agent turns this into a task.</p>` : '';
  main.innerHTML = `<h1>Plan — ${esc(model.display_name)}</h1>
    <p class="muted">Tasks are sized by estimated lines of code. <a href="#/decisions/${esc(repoId)}">Decisions →</a></p>
    ${requestBlock}
    ${!model.releases.length && !model.tasks.length ? stateBlock('empty', 'No plan yet. The agent will publish tasks and releases here once planning starts.') : gantt}
    ${model.tasks_truncated ? '<p class="muted">Only the newest finished tasks are shown; everything stays permanently recorded.</p>' : ''}
    ${commentForm}`;
  bind(main);
  bindMoveButtons(main, model);
  main.querySelectorAll('[data-collapse]').forEach((btn) => btn.addEventListener('click', () => {
    const id = btn.dataset.collapse;
    if (state.collapsed.has(id)) state.collapsed.delete(id); else state.collapsed.add(id);
    render();
  }));
  if (admin) bindGanttDrag(main, model);
  $('#comment-form')?.addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const fd = new FormData(ev.target);
    const args = { repository_id: repoId, title: fd.get('title'), kind: 'user_feedback' };
    if (fd.get('impact')) args.impact = fd.get('impact');
    await act(ev.target.querySelector('button[type=submit]'), 'task.create', args, () => render());
  });
});

// Pop-up move/reorder — the touch and accessibility path beside drag-and-drop.
function openMoveDialog(task, model) {
  document.getElementById('move-dialog')?.remove();
  const open = model.releases.filter((r) => r.status !== 'delivered');
  const current = model.releases.find((r) => r.release_id === task.release_id);
  const dlg = document.createElement('dialog');
  dlg.id = 'move-dialog';
  dlg.innerHTML = `<h2>Move: ${esc(task.title)}</h2>
    <p class="muted">Now in ${esc(current ? current.name : 'not scheduled yet')}.</p>
    <form id="move-form" class="inline">
      <label class="f">move to<select name="release_id">${open.map((r) => `<option value="${esc(r.release_id)}"${r.release_id === task.release_id ? ' selected' : ''}>${esc(r.name)}</option>`).join('')}<option value=""${task.release_id ? '' : ' selected'}>not scheduled yet (backlog)</option></select></label>
      <label class="f">put first<input type="checkbox" name="first"></label>
      <div class="actions" style="flex-basis:100%">
        <button class="btn" type="submit">Move</button>
        <button class="btn" type="button" id="move-cancel">Cancel</button>
      </div>
    </form>`;
  document.body.appendChild(dlg);
  dlg.addEventListener('close', () => dlg.remove());
  $('#move-cancel', dlg).addEventListener('click', () => dlg.close());
  $('#move-form', dlg).addEventListener('submit', async (ev) => {
    ev.preventDefault();
    const fd = new FormData(ev.target);
    const target = fd.get('release_id') || null;
    const args = { task_id: task.task_id };
    if (target !== (task.release_id || null)) args.release_id = target;
    if (fd.get('first') === 'on') args.position = 0;
    if (!('release_id' in args) && !('position' in args)) { dlg.close(); return; }
    await act(ev.target.querySelector('button[type=submit]'), 'task.update', args, () => { dlg.close(); return render(); });
  });
  dlg.showModal();
}
function bindMoveButtons(root, model) {
  root.querySelectorAll('[data-move-task]').forEach((btn) => btn.addEventListener('click', () => {
    const t = model.tasks.find((x) => x.task_id === btn.dataset.moveTask);
    if (t) openMoveDialog(t, model);
  }));
}

// Native HTML5 drag-and-drop: reorder within a release, move across releases
// (drop between rows), or drop on a release header to append to it.
let dragTaskId = null;
function bindGanttDrag(root, model) {
  const known = new Set(model.releases.map((r) => r.release_id));
  const groupOf = (t) => (t.release_id && known.has(t.release_id) ? t.release_id : null);
  const byId = new Map(model.tasks.map((t) => [t.task_id, t]));
  const clearMarks = () => root.querySelectorAll('.gdrop-before,.gdrop-after,.gdrop-into').forEach((el) => el.classList.remove('gdrop-before', 'gdrop-after', 'gdrop-into'));
  const move = async (args) => {
    try { await api('task.update', args); toast('task moved', 'ok'); render(); }
    catch (e) { toast(`move failed: ${e.message}`, 'bad'); }
  };
  root.querySelectorAll('[data-drag-task]').forEach((row) => {
    row.addEventListener('dragstart', (ev) => {
      dragTaskId = row.dataset.dragTask;
      ev.dataTransfer.effectAllowed = 'move';
      ev.dataTransfer.setData('text/plain', dragTaskId);
    });
    row.addEventListener('dragend', () => { dragTaskId = null; clearMarks(); });
  });
  root.querySelectorAll('[data-task-row]').forEach((row) => {
    row.addEventListener('dragover', (ev) => {
      if (!dragTaskId || dragTaskId === row.dataset.taskRow) return;
      ev.preventDefault();
      clearMarks();
      row.classList.add(ev.offsetY < row.offsetHeight / 2 ? 'gdrop-before' : 'gdrop-after');
    });
    row.addEventListener('drop', (ev) => {
      if (!dragTaskId || dragTaskId === row.dataset.taskRow) return;
      ev.preventDefault();
      const before = ev.offsetY < row.offsetHeight / 2;
      const target = byId.get(row.dataset.taskRow);
      const dragged = byId.get(dragTaskId);
      clearMarks(); dragTaskId = null;
      if (!target || !dragged) return;
      const siblings = model.tasks
        .filter((t) => (t.parent_task_id || null) === (target.parent_task_id || null) && groupOf(t) === groupOf(target) && t.task_id !== dragged.task_id)
        .sort((a, b) => (a.position - b.position) || (a.seq - b.seq));
      const idx = siblings.findIndex((t) => t.task_id === target.task_id);
      const args = { task_id: dragged.task_id, position: Math.max(0, before ? idx : idx + 1) };
      if ((target.release_id || null) !== (dragged.release_id || null)) args.release_id = target.release_id || null;
      if ((target.parent_task_id || null) !== (dragged.parent_task_id || null)) args.parent_task_id = target.parent_task_id || null;
      move(args);
    });
  });
  root.querySelectorAll('[data-drop-release]').forEach((head) => {
    head.addEventListener('dragover', (ev) => { if (!dragTaskId) return; ev.preventDefault(); clearMarks(); head.classList.add('gdrop-into'); });
    head.addEventListener('dragleave', () => head.classList.remove('gdrop-into'));
    head.addEventListener('drop', (ev) => {
      if (!dragTaskId) return;
      ev.preventDefault();
      const dragged = byId.get(dragTaskId);
      const target = head.dataset.dropRelease || null;
      clearMarks(); dragTaskId = null;
      if (!dragged) return;
      if ((dragged.release_id || null) === target && !dragged.parent_task_id) return;
      const args = { task_id: dragged.task_id, release_id: target };
      if (dragged.parent_task_id) args.parent_task_id = null;
      move(args);
    });
  });
}

// --- Decisions -------------------------------------------------------------
const ASPECTS = ['all', 'ui', 'architecture', 'algorithms', 'business_logic', 'data', 'testing', 'deployment', 'security', 'performance', 'process', 'other'];
const paragraphs = (text) => String(text).split(/\n+/).filter(Boolean).map((p) => `<p>${esc(p)}</p>`).join('');
const viewDecisions = guard(async (repoId) => {
  main.innerHTML = `<h1>Decisions</h1>${skeleton(5)}`;
  const aspect = state.decisionAspect !== 'all' ? { aspect: state.decisionAspect } : {};
  const searching = !!state.decisionQuery;
  const result = searching
    ? await api('decision.search', { repository_id: repoId, query: state.decisionQuery, n: state.decisionLimit, ...aspect })
    : await api('decision.tail', { repository_id: repoId, n: state.decisionLimit, ...aspect, ...(state.decisionBefore ? { before_seq: state.decisionBefore } : {}) });
  const entries = searching ? result.decisions : [...result.decisions].reverse();
  const card = (d) => {
    const head = `<strong>${esc(d.title)}</strong> ${badge(d.aspect.replace('_', ' '))}${d.ref ? ` <span class="muted mono">${esc(d.ref)}</span>` : ''} <span class="muted">${ago(d.created_at)}</span>`;
    if (d.superseded_by) return `<details class="decision superseded"><summary>${head} ${badge('superseded')}</summary>${paragraphs(d.body)}</details>`;
    return `<div class="decision">${head}${paragraphs(d.body)}</div>`;
  };
  const story = !searching && !state.decisionBefore
    ? `<div class="story"><h2>The story so far</h2>${result.summary ? paragraphs(result.summary.body) : '<p class="muted">No summary yet.</p>'}${result.summary_due ? '<p class="muted">The agent will refresh this summary soon.</p>' : ''}</div>` : '';
  const emptyText = searching ? 'Nothing found for that search.'
    : state.decisionAspect !== 'all' ? `No ${state.decisionAspect.replace('_', ' ')} decisions yet.`
      : 'No decisions yet. The agent records its choices here as it works.';
  main.innerHTML = `<h1>Decisions — ${esc(result.display_name || '')}</h1>
    <p class="muted">Recorded choices in plain language. <a href="#/plan/${esc(repoId)}">Plan →</a></p>
    ${story}
    <form class="inline" id="decision-search"><label class="f">search every decision<input name="q" value="${esc(state.decisionQuery)}" placeholder="e.g. why exports are files"></label><button class="btn" type="submit">Search</button>${searching || state.decisionBefore ? '<button class="btn" type="button" id="decisions-latest">Show latest</button>' : ''}</form>
    <div class="segwrap">${seg(ASPECTS, state.decisionAspect, 'decision-aspect', (o) => o.replace('_', ' '))}</div>
    ${entries.length ? entries.map(card).join('') : stateBlock('empty', emptyText)}
    ${!searching && result.has_more ? '<p><button class="btn" id="decisions-older">Show older decisions</button></p>' : ''}`;
  bindSeg(main, 'decision-aspect', (a) => { state.decisionAspect = a; state.decisionBefore = null; render(); });
  $('#decision-search').addEventListener('submit', (ev) => {
    ev.preventDefault();
    state.decisionQuery = String(new FormData(ev.target).get('q') || '').trim();
    state.decisionBefore = null;
    render();
  });
  $('#decisions-latest')?.addEventListener('click', () => { state.decisionQuery = ''; state.decisionBefore = null; render(); });
  $('#decisions-older')?.addEventListener('click', () => {
    if (entries.length) state.decisionBefore = entries[entries.length - 1].seq;
    render();
  });
});

// --- Router ----------------------------------------------------------------
async function render() {
  viewAbort?.abort();
  viewAbort = new AbortController();
  const hash = location.hash || '#/deployments';
  const [, view, arg] = hash.slice(1).split('/');
  document.querySelectorAll('#nav a').forEach((a) => a.classList.toggle('active', a.dataset.view === view));
  setBanner('');
  if (view === 'deployments') return arg ? viewDeployment(arg) : viewDeployments();
  if (view === 'plan') return arg ? viewPlan(arg) : viewPlanPicker('plan');
  if (view === 'decisions') return arg ? viewDecisions(arg) : viewPlanPicker('decisions');
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
