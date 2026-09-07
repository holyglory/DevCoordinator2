import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { createEdge, loadConfig } from '../devcoordinator2-edge.mjs';

test('base redirect is opt-in, exact-host-only, and precedes authentication handling', async (context) => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-base-redirect-'));
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const environment = { EDGE_BASE_DOMAIN: 'example.test', EDGE_HTTP_ONLY: '1', EDGE_HTTP_PORT: '0',
    EDGE_LISTEN_HOST: '127.0.0.1', EDGE_SESSION_SECRET: 'fixture-session-secret-long-enough',
    EDGE_ROUTES_FILE: path.join(directory, 'missing-routes.json'), EDGE_STATE_DIR: path.join(directory, 'state') };
  assert.equal(loadConfig(environment).baseRedirect, false);
  async function start(config) {
    const edge = await createEdge(config, { log: { info() {}, warn() {}, error() {} } });
    context.after(() => edge.close());
    const [port] = await edge.listen();
    return { edge, port };
  }
  async function request(port, pathname, host = 'example.test') {
    return new Promise((resolve, reject) => {
      const outgoing = http.get({ host: '127.0.0.1', port, path: pathname, headers: { host } }, (response) => {
        response.resume();
        response.on('end', () => resolve({ status: response.statusCode, location: response.headers.location }));
      });
      outgoing.on('error', reject);
    });
  }
  const disabled = await start(loadConfig(environment));
  assert.equal((await request(disabled.port, '/')).status, 404);
  const enabled = await start(loadConfig({ ...environment, EDGE_BASE_REDIRECT: '1', EDGE_STATE_DIR: path.join(directory, 'enabled') }));
  for (const pathname of ['/', '/dashboard?filter=a%20b&sort=desc', '/auth/start?rt=%2Fdashboard']) {
    assert.deepEqual(await request(enabled.port, pathname), {
      status: 301, location: `${enabled.edge.consoleOrigin}${pathname}`,
    });
  }
  for (const host of ['unknown.example.test', 'probe.example.test', 'example.test.outside.test', 'outside.test']) {
    assert.deepEqual(await request(enabled.port, '/', host), { status: 404, location: undefined });
  }
  assert.deepEqual(await request(enabled.port, '/healthz', 'console.example.test'), { status: 200, location: undefined });
  const absolute = await request(enabled.port, 'http://outside.test/path?query=1');
  assert.equal(absolute.location, `${enabled.edge.consoleOrigin}/path?query=1`);
});
