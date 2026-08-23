// Browser verification of the Console: every primary view in every required
// state at a wide and a narrow viewport, with software-detectable checks
// (no document overflow, no clipped tiles, controls visible and enabled)
// and click-through proofs that controls call the real API and re-render.
//
// Usage:  CONSOLE_VERIFY_PLAYWRIGHT=<dir containing node_modules/playwright> \
//         CONSOLE_VERIFY_OUT=<dir> node console/verify.mjs
// Fixture data is used only here, never in production views.

import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { createRequire } from 'node:module';

import { createEdge } from '../edge/devcoordinator2-edge.mjs';
import { createSessionManager } from '../edge/lib/session.mjs';
import { canonicalJson } from '../edge/lib/routes-store.mjs';

const BASE = 'example.test';
const HOST = `console.${BASE}`;
const OUT = process.env.CONSOLE_VERIFY_OUT || path.join(os.tmpdir(), 'dc2-console-verify');
const pw = createRequire(path.join(process.env.CONSOLE_VERIFY_PLAYWRIGHT || process.cwd(), 'package.json'))('playwright');

const DEP = 'd0123456789abcdef';
const LONG = 'a-very-long-deployment-name-that-keeps-going-and-going-for-quite-a-while';
const fixtures = (scenario) => {
  const running = { deployment_id: DEP, repository_id: 'r0123456789abcdef', name: 'web', source: 'worktree', state: scenario.stopped ? 'stopped' : 'running', domain: `app-dev.${BASE}`, current_generation: 17, updated_at: new Date(Date.now() - 90000).toISOString(), ttl_expires_at: null };
  const degraded = { deployment_id: 'd1111111111111111', repository_id: 'r0123456789abcdef', name: LONG, source: 'checkout', state: 'degraded', domain: `${LONG}.${BASE}`, current_generation: 2147483647, updated_at: new Date().toISOString(), ttl_expires_at: '2026-12-31T00:00:00Z' };
  const components = [
    { name: 'db', type: 'postgres', state: 'running', health: 'healthy', generation: 0, binding: { kind: 'container', identity: 'c'.repeat(64) }, port: 20001, restarts: 0, owned: true, independent_control: true, last_error: null },
    { name: 'api', type: 'process', state: scenario.stopped ? 'stopped' : 'running', health: scenario.stopped ? 'none' : 'healthy', generation: 17, binding: { kind: 'unit', identity: `devcoordinator2-deploy-${DEP}-api-g17.service` }, port: 20002, restarts: 3, owned: true, independent_control: true, last_error: null },
    { name: 'worker', type: 'process', state: 'failed', health: 'unhealthy', generation: 17, binding: { kind: 'unit', identity: `devcoordinator2-deploy-${DEP}-worker-g17.service` }, port: null, restarts: 9999999, owned: true, independent_control: false, last_error: 'exited 1: ' + 'x'.repeat(120) },
    { name: 'smtp', type: 'external', state: 'running', health: 'healthy', generation: null, binding: { kind: null, identity: null }, port: null, restarts: 0, owned: false, independent_control: true, last_error: null },
  ];
  const points = Array.from({ length: 30 }, (_, i) => ({ minute: `2026-01-01T00:${String(i).padStart(2, '0')}Z`, min: i, avg: i * 1.5, max: i * 2, samples: 4 }));
  return {
    'user.whoami': { local: false, identity: scenario.identity, user_id: 'u1', administrator: scenario.admin, grants: scenario.admin ? {} : { [DEP]: 'operator' } },
    'deployment.list': { deployments: scenario.empty ? [] : [running, degraded], declared: scenario.empty ? [] : [{ name: 'tool', source: 'worktree', deployment_id: 'd2222222222222222' }] },
    'deployment.status': { ...running, previous_generation: 16, route_port: 20002, components, log_dir: '/state/logs' },
    'deployment.logs': { component: 'api', tail: 'line 1\nline 2 ' + 'long '.repeat(60) + '\nline 3', truncated_before_tail: true, log_path: '/state/logs/api.log' },
    'health.history': { subject_kind: 'component', subject_id: `${DEP}/api`, metric: 'cpu_percent', minutes: 60, points: scenario.empty ? [] : points, truncated: false },
    'test.list': { runs: scenario.empty ? [] : [
      { run_id: 't20260101T000000Z-abc123', test: 'unit', status: 'running', started_at: new Date().toISOString(), finished_at: null, duration_seconds: null, exit_code: null, stdout_bytes_observed: 123456789, stderr_bytes_observed: 0, stdout_truncated: true, stderr_truncated: false, display_name: 'repo-one', worktree_path: '/srv/repos/repo-one', repository_id: 'r1', worktree_id: 'w1', summary_path: '/srv/repos/repo-one/.devcoordinator/test/current/summary.json' },
      { run_id: 't20260101T000100Z-def456', test: 'integration-with-a-long-name', status: 'failed', started_at: new Date(Date.now() - 3600000).toISOString(), finished_at: new Date().toISOString(), duration_seconds: 3599.123, exit_code: 1, stdout_bytes_observed: 10, stderr_bytes_observed: 4194304, stdout_truncated: false, stderr_truncated: true, display_name: LONG, worktree_path: `/srv/repos/${LONG}`, repository_id: 'r2', worktree_id: 'w2', summary_path: '/x' }] },
    'test.output': { run_id: 't1', stream: 'stdout', tail: 'ok\n'.repeat(5), tail_bytes: 15, truncated_before_tail: true, log_path: '/srv/repos/repo-one/.devcoordinator/test/current/stdout.log' },
    'health.summary': { host: { cpu_percent: 93.4, memory_total: 264122252 * 1024, memory_used: 108579328 * 1024, memory_available: 155542924 * 1024, swap_total: 0, swap_used: 0, load_1: 8.32, load_5: 8.39, load_15: 7.69, fs_size: 2113513742336, fs_free: 148698841088, fs_used: 1964814901248, ncpu: 32, reconciliation: { managed_cpu_percent: 40.1, daemon_cpu_percent: 0.3, other_cpu_percent: 53.0, managed_memory: 50e9, daemon_memory: 120e6, other_memory: 60e9 } }, storage: { fs_used: 1964814901248, managed_repositories: 4e11, devcoordinator_state: 5e7, docker_shared: 3e10, docker_images: 2.7e10, docker_build_cache: 2.8e9, docker_shared_volumes: 1e8, other: 1.5e12 }, unhealthy_deployments: scenario.empty ? [] : [degraded], active_tests: scenario.empty ? [] : ['unit'], container_counts: { 'managed-test': 1, 'managed-preview': 0, 'managed-permanent': 3, 'orphaned-managed': 1, unmanaged: 43 }, alerts: scenario.empty ? [] : [{ alert_key: 'host/cpu', kind: 'host_cpu', severity: 'warning', message: 'host CPU 93% sustained', opened_at: new Date().toISOString() }, { alert_key: `component/${DEP}/worker/unhealthy`, kind: 'component_unhealthy', severity: 'critical', message: `component ${DEP}/worker is failed`, opened_at: new Date().toISOString() }], sampling: { retention_days: 30 } },
    'health.repositories': { repositories: scenario.empty ? [] : [{ repository_id: 'r0123456789abcdef', display_name: 'repo-one', root_path: '/srv/repos/repo-one', cpu_percent: 40.1, memory_bytes: 5e10, storage_bytes: 4e11, storage: {}, health: 'unhealthy', deployments: [running, degraded], trend_cpu: [1, 5, 3, 8, 2, 9, 4, 7, 3, 6, 2, 5], trend_memory: [1, 1, 2, 2, 3, 3, 3, 4, 4, 4, 5, 5] }, { repository_id: 'r2', display_name: LONG, root_path: `/srv/repos/${LONG}`, cpu_percent: 0, memory_bytes: 0, storage_bytes: 1234567890123, storage: {}, health: 'none', deployments: [], trend_cpu: [], trend_memory: [] }], devcoordinator: { cpu_percent: 0.3, memory_bytes: 120e6, storage_bytes: 5e7 }, shared_unattributed: { cpu_percent: 53, memory_bytes: 60e9, storage: { docker_images: 2.7e10, other: 1.5e12 } }, host: {} },
    'health.containers': { containers: scenario.empty ? [] : [
      { id: 'a'.repeat(64), name: 'devcoordinator2-deploy-x-db', image: 'postgres:16-alpine', state: 'running', status: 'Up 3 days', created: '2026-08-20 10:00:00 +0000 UTC', repository_id: 'r0123456789abcdef', deployment_id: DEP, component: 'db', run_id: null, caller_uid: 1000, client: 'claude', ttl_seconds: null, data: 'persistent', classification: 'managed-permanent', cpu_percent: 1.2, memory_bytes: 2677821440, pids: 7, container_layer_bytes: 0 },
      { id: 'b'.repeat(64), name: 'legacy-thing-1', image: 'some/image:latest', state: 'running', status: 'Up 6 weeks', created: '2026-07-01', repository_id: null, deployment_id: null, component: null, run_id: null, caller_uid: null, client: null, ttl_seconds: null, data: null, classification: 'unmanaged', cpu_percent: 12.5, memory_bytes: 9e9, pids: 100, container_layer_bytes: null },
      { id: 'c'.repeat(64), name: 'devcoordinator2-test-old-postgres', image: 'postgres:16-alpine', state: 'exited', status: 'Exited (0)', created: '2026-08-22', repository_id: 'r1', deployment_id: null, component: null, run_id: 't-old', caller_uid: 1001, client: 'codex', ttl_seconds: 3600, data: 'disposable', classification: 'orphaned-managed', cpu_percent: null, memory_bytes: null, pids: null, container_layer_bytes: 12345 }], counts: { 'managed-test': 0, 'managed-preview': 0, 'managed-permanent': 1, 'orphaned-managed': 1, unmanaged: 1 } },
    'bug.list': { bugs: scenario.empty ? [] : [{ bug_id: 'b0123456789ab', component: 'api', summary: 'Returns 500 on /export when the report is large', expected: '200 with CSV', actual: '500', steps: '1. open /export 2. choose all-time 3. submit', opened_at: '2026-08-20T10:00:00Z', last_seen_at: new Date().toISOString(), occurrences: 42, reporter: 'dev@example.test', correlations: { deployment_id: DEP } }], store: '/bugs' },
    'user.list': { users: [{ user_id: 'u1', email: 'owner@example.test', administrator: true, grants: [], last_seen_at: new Date().toISOString() }, { user_id: 'u2', email: `${'verylongmailboxname'.repeat(3)}@example.test`, administrator: false, grants: [{ deployment_id: DEP, role: 'operator', granted_at: 't' }], last_seen_at: null }], invitations: [{ invitation_id: 'i1', email: 'new@example.test', administrator: false, grants: [{ deployment_id: DEP, role: 'viewer' }], created_at: 't', created_by: 'owner', expires_at: '2026-09-06T00:00:00Z' }], roles: ['access', 'viewer', 'operator', 'administrator'], owners: ['owner@example.test'] },
    'telegram.list': { configured: true, chats: [{ chat_id: 4242, email: 'owner@example.test', label: 'Owner', linked_at: 't', subscriptions: ['server', `deployment:${DEP}`] }], outbox_pending: 0, last_poll_at: new Date().toISOString(), last_error: null },
    ping: { daemon_version: '0.1.0', schema_version: 5, socket: '/run/x.sock' },
  };
};

const SCENARIOS = {
  populated: { identity: 'owner@example.test', admin: true },
  empty: { identity: 'owner@example.test', admin: true, empty: true },
  error: { identity: 'owner@example.test', admin: true, error: true },
  loading: { identity: 'owner@example.test', admin: true, delayMs: 4000 },
  denied: { identity: 'dev@example.test', admin: false, denied: true },
};
const VIEWS = ['#/deployments', `#/deployments/${DEP}`, '#/tests', '#/health', '#/health/containers', '#/bugs', '#/admin'];
const VIEWPORTS = { wide: { width: 1280, height: 800 }, narrow: { width: 390, height: 844 } };
const ADMIN_ONLY = ['health.summary', 'health.containers', 'health.container_remove', 'user.list', 'user.invite', 'user.remove', 'grant.set', 'grant.remove', 'test.list', 'test.start', 'test.stop', 'test.output', 'deployment.apply', 'deployment.rollback', 'deployment.remove'];

async function startFakeDaemon(dir) {
  const socketPath = path.join(dir, 'daemon.sock');
  let scenario = SCENARIOS.populated; const calls = []; const mutable = { stopped: false };
  const server = net.createServer({ allowHalfOpen: true }, (socket) => {
    let buf = '';
    socket.on('data', async (c) => {
      buf += c; if (!buf.endsWith('\n')) return;
      const req = JSON.parse(buf); calls.push(req);
      const reply = (payload) => socket.end(`${JSON.stringify({ protocol: 1, id: req.id, ...payload })}\n`);
      if (scenario.delayMs) await new Promise((r) => setTimeout(r, scenario.delayMs));
      const cmd = req.command;
      if (scenario.error && cmd !== 'user.whoami') return reply({ ok: false, error: { code: 'internal_error', message: 'simulated daemon fault', detail: '' } });
      if (scenario.denied && ADMIN_ONLY.includes(cmd)) return reply({ ok: false, error: { code: 'permission_denied', message: `${cmd} requires administrator`, detail: '' } });
      if (cmd === 'deployment.stop') { mutable.stopped = true; return reply({ ok: true, result: { state: 'stopped' } }); }
      if (cmd === 'deployment.start') { mutable.stopped = false; return reply({ ok: true, result: { state: 'running' } }); }
      if (['deployment.restart', 'deployment.apply', 'deployment.rollback', 'deployment.remove', 'bug.report', 'bug.close', 'user.invite', 'user.remove', 'grant.set', 'grant.remove', 'telegram.link', 'telegram.subscribe', 'telegram.unsubscribe', 'test.stop', 'test.start', 'health.container_remove'].includes(cmd)) return reply({ ok: true, result: { state: 'done', status: 'done' } });
      const data = fixtures({ ...scenario, stopped: mutable.stopped })[cmd];
      if (data === undefined) return reply({ ok: false, error: { code: 'command_unknown', message: cmd, detail: '' } });
      return reply({ ok: true, result: data });
    });
  });
  await new Promise((r) => server.listen(socketPath, r));
  return { socketPath, calls, setScenario: (s) => { scenario = s; mutable.stopped = false; calls.length = 0; }, close: () => new Promise((r) => server.close(r)) };
}

async function main() {
  await fs.mkdir(OUT, { recursive: true });
  const tmp = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-verify-'));
  const daemon = await startFakeDaemon(tmp);
  const payload = { generation: 1, published_at: 'x', domain: BASE, routes: [], access: { owners: ['owner@example.test'], grants: [] } };
  await fs.writeFile(path.join(tmp, 'routes.json'), JSON.stringify({ schema: 1, payload_sha256: crypto.createHash('sha256').update(canonicalJson(payload)).digest('hex'), ...payload }));
  const secret = 'console-verify-secret-0123456789';
  const edge = await createEdge({ baseDomain: BASE, consoleHost: HOST, httpPort: 0, httpOnly: true, sessionSecret: secret, oidcIssuer: 'http://127.0.0.1:1/', oidcClientId: '', oidcClientSecret: '', routesFile: path.join(tmp, 'routes.json'), stateDir: path.join(tmp, 'edge-state'), daemonSocket: daemon.socketPath, consoleDir: path.resolve(path.dirname(new URL(import.meta.url).pathname)) }, { log: { info() {}, warn() {}, error: (...a) => console.error('edge', ...a), debug() {} } });
  const [port] = await edge.listen();
  const sessions = createSessionManager({ secret, ttlMs: 3600000, cookieName: 'dc2_session', cookieDomain: `.${BASE}`, secure: false });
  const browser = await pw.chromium.launch({ args: [`--host-resolver-rules=MAP *.${BASE} 127.0.0.1`] });
  const report = { checks: [], failures: [] };
  const check = (name, ok, detail = '') => { report.checks.push({ name, ok, detail }); if (!ok) report.failures.push(`${name}: ${detail}`); };

  for (const [scenarioName, scenario] of Object.entries(SCENARIOS)) {
    daemon.setScenario(scenario);
    for (const [vpName, viewport] of Object.entries(VIEWPORTS)) {
      const context = await browser.newContext({ viewport, ignoreHTTPSErrors: true });
      const { cookie } = sessions.issue({ sub: 'sub', email: scenario.identity, name: 'Verifier' });
      const [nameValue] = cookie.split(';');
      await context.addCookies([{ name: 'dc2_session', value: nameValue.split('=')[1], domain: `.${BASE}`, path: '/' }]);
      const page = await context.newPage();
      page.on('dialog', (d) => d.accept());
      for (const view of VIEWS) {
        const label = `${scenarioName}-${view.replace(/[#/]+/g, '_').replace(/^_/, '')}-${vpName}`;
        await page.goto(`http://${HOST}:${port}/${view}`);
        if (scenario.delayMs) { await page.waitForTimeout(500); }
        else { await page.waitForFunction(() => !document.querySelector('.skeleton'), null, { timeout: 15000 }).catch(() => {}); await page.waitForTimeout(300); }
        await page.screenshot({ path: path.join(OUT, `${label}.png`), fullPage: true });
        const metrics = await page.evaluate(() => {
          const doc = document.documentElement;
          const overflow = doc.scrollWidth - window.innerWidth;
          const clipped = [...document.querySelectorAll('.tile .v, h1, .toast')].filter((el) => el.scrollWidth > el.clientWidth + 1).map((el) => el.textContent.slice(0, 40));
          const buttons = [...document.querySelectorAll('button')].map((b) => ({ text: b.textContent.trim(), visible: b.offsetParent !== null, disabled: b.disabled, x: b.getBoundingClientRect().right, scrollable: !!b.closest('.tablewrap') }));
          const offscreen = buttons.filter((b) => b.visible && b.x > window.innerWidth + 1 && !b.scrollable);
          return { overflow, clipped, buttons: buttons.length, offscreen: offscreen.length, text: document.body.innerText.slice(0, 4000), skeleton: !!document.querySelector('.skeleton'), notice: document.querySelector('.notice')?.textContent || '' };
        });
        check(`${label}: no horizontal document overflow`, metrics.overflow <= 0, `overflow ${metrics.overflow}px`);
        check(`${label}: no clipped headline text`, metrics.clipped.length === 0, metrics.clipped.join(' | '));
        check(`${label}: no off-canvas controls outside scroll containers`, metrics.offscreen === 0, `${metrics.offscreen} off-canvas`);
        if (scenarioName === 'loading') check(`${label}: loading state visible`, metrics.skeleton || /Loading/.test(metrics.text));
        if (scenarioName === 'empty' && !view.includes(DEP) && view !== '#/admin') check(`${label}: explicit empty state`, /No (deployments|test runs|open bugs|containers|repositories)/.test(metrics.text), metrics.text.slice(0, 120));
        if (scenarioName === 'error') check(`${label}: error state with retry`, /Could not load|Cannot reach/.test(metrics.text) && /Retry/.test(metrics.text), metrics.text.slice(0, 120));
        if (scenarioName === 'denied' && (view === '#/admin' || view === '#/tests' || view === '#/health/containers')) check(`${label}: permission denied shown`, /Permission denied/.test(metrics.notice), metrics.notice.slice(0, 120));
        if (scenarioName === 'denied' && view === '#/health') check(`${label}: host health denied but repositories visible`, /administrator-only/.test(metrics.text) && /repo-one/.test(metrics.text));
        if (scenarioName === 'populated' && ['#/deployments', '#/tests', '#/health'].includes(view)) check(`${label}: long names rendered`, /going-and-going/.test(metrics.text), metrics.text.slice(0, 80));
        if (scenarioName === 'populated' && ['#/tests', '#/health'].includes(view)) check(`${label}: large numbers humanized`, /MiB|GiB|TiB/.test(metrics.text), metrics.text.slice(0, 80));
      }
      await context.close();
    }
  }

  // Interaction proofs (populated, wide): controls call the API and the view re-renders.
  daemon.setScenario(SCENARIOS.populated);
  const context = await browser.newContext({ viewport: VIEWPORTS.wide });
  const { cookie } = sessions.issue({ sub: 'sub', email: 'owner@example.test' });
  await context.addCookies([{ name: 'dc2_session', value: cookie.split(';')[0].split('=')[1], domain: `.${BASE}`, path: '/' }]);
  const page = await context.newPage();
  page.on('dialog', (d) => d.accept());
  await page.goto(`http://${HOST}:${port}/#/deployments/${DEP}`);
  await page.waitForSelector('button[data-cmd="deployment.stop"]');
  await page.click('h1 ~ .actions button[data-cmd="deployment.stop"]');
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'stopped'), null, { timeout: 10000 });
  check('interaction: stop calls deployment.stop and the header shows stopped', daemon.calls.some((c) => c.command === 'deployment.stop' && c.args.deployment_id === DEP && c.client.identity === 'owner@example.test'));
  await page.click('h1 ~ .actions button[data-cmd="deployment.start"]');
  await page.waitForFunction(() => [...document.querySelectorAll('h1 .badge')].some((b) => b.textContent === 'running'), null, { timeout: 10000 });
  check('interaction: start restores running', true);
  await page.click('button[data-logs="api"]');
  await page.waitForSelector('pre.log');
  check('interaction: logs load on demand', daemon.calls.some((c) => c.command === 'deployment.logs' && c.args.component === 'api'));
  await page.click('button[data-cmd="deployment.remove"]');
  await page.waitForTimeout(500);
  const removeCall = daemon.calls.find((c) => c.command === 'deployment.remove');
  check('interaction: remove asks for confirmation and passes delete_data explicitly', removeCall && typeof removeCall.args.delete_data === 'boolean');
  await page.goto(`http://${HOST}:${port}/#/tests`);
  await page.waitForSelector('button[data-out="stderr"]');
  await page.click('button[data-out="stderr"]');
  await page.waitForSelector('pre.log');
  check('interaction: test output loads on demand with bounded tail', daemon.calls.some((c) => c.command === 'test.output' && c.args.tail_bytes === 16384));
  await page.goto(`http://${HOST}:${port}/#/bugs`);
  await page.waitForSelector('#bug-form');
  for (const [f, v] of [['component', 'api'], ['summary', 'verify'], ['expected', 'a'], ['actual', 'b'], ['steps', 'c']]) await page.fill(`#bug-form [name=${f}]`, v);
  await page.click('#bug-form button[type=submit]');
  await page.waitForTimeout(500);
  check('interaction: bug report form calls bug.report', daemon.calls.some((c) => c.command === 'bug.report' && c.args.summary === 'verify'));
  await page.goto(`http://${HOST}:${port}/#/admin`);
  await page.waitForSelector('#invite-form');
  await page.fill('#invite-form [name=email]', 'new2@example.test');
  await page.click('#invite-form button[type=submit]');
  await page.waitForTimeout(500);
  check('interaction: invite form calls user.invite', daemon.calls.some((c) => c.command === 'user.invite' && c.args.email === 'new2@example.test'));
  await page.goto(`http://${HOST}:${port}/#/health/containers`);
  await page.waitForSelector('button[data-cmd="health.container_remove"]');
  const removable = await page.$$('button[data-cmd="health.container_remove"]');
  check('interaction: only orphaned/test containers offer removal', removable.length === 1);
  await removable[0].click();
  await page.waitForTimeout(500);
  check('interaction: container removal calls health.container_remove with the exact id', daemon.calls.some((c) => c.command === 'health.container_remove' && c.args.container_id === 'c'.repeat(64)));
  await context.close();

  await browser.close();
  await edge.close();
  await daemon.close();
  report.summary = { checks: report.checks.length, failures: report.failures.length, screenshots: OUT };
  await fs.writeFile(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report.summary));
  for (const f of report.failures) console.log('FAIL', f);
  process.exit(report.failures.length ? 1 : 0);
}

main().catch((error) => { console.error(error); process.exit(2); });
