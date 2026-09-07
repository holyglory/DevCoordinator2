import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { createEdge, loadConfig } from '../devcoordinator2-edge.mjs';

test('optional listen host binds the actual canary listener to IPv4 loopback', async (context) => {
  assert.equal(loadConfig({ EDGE_BASE_DOMAIN: 'example.test' }).listenHost, '');
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-listen-host-'));
  context.after(() => fs.rm(directory, { recursive: true, force: true }));
  const servers = [];
  const originalCreateServer = http.createServer;
  context.mock.method(http, 'createServer', (...args) => {
    const server = originalCreateServer(...args);
    servers.push(server);
    return server;
  });
  const config = loadConfig({ EDGE_BASE_DOMAIN: 'example.test', EDGE_HTTP_ONLY: '1',
    EDGE_HTTP_PORT: '0', EDGE_LISTEN_HOST: '127.0.0.1',
    EDGE_SESSION_SECRET: 'fixture-session-secret-long-enough',
    EDGE_ROUTES_FILE: path.join(directory, 'missing-routes.json'), EDGE_STATE_DIR: path.join(directory, 'state') });
  const edge = await createEdge(config, { log: { info() {}, warn() {}, error() {} } });
  context.after(() => edge.close());
  const [port] = await edge.listen();
  assert.equal(servers.length, 1);
  assert.deepEqual(servers[0].address(), { address: '127.0.0.1', family: 'IPv4', port });
  const status = await new Promise((resolve, reject) => {
    const request = http.get({ host: '127.0.0.1', port, path: '/healthz', headers: { host: 'console.example.test' } }, (response) => {
      response.resume();
      response.on('end', () => resolve(response.statusCode));
    });
    request.on('error', reject);
  });
  assert.equal(status, 200);
});
