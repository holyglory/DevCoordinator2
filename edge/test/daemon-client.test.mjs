import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';

import { createDaemonClient } from '../lib/daemon-client.mjs';

async function fixture(respond, invoke) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-daemon-client-'));
  const socketPath = path.join(directory, 'daemon.sock');
  const server = net.createServer((socket) => {
    const chunks = [];
    socket.on('data', (chunk) => chunks.push(chunk));
    socket.on('end', () => respond(socket, Buffer.concat(chunks)));
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  try {
    return await invoke(createDaemonClient({ socketPath, timeoutMs: 1000 }));
  } finally {
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  }
}

test('daemon client emits and accepts only the strict protocol-v2 envelope', async () => {
  const response = await fixture((socket, raw) => {
    assert.equal(raw.at(-1), 0x0a);
    const request = JSON.parse(raw.subarray(0, -1));
    assert.deepEqual(Object.keys(request).sort(), ['client', 'id', 'operation', 'params', 'protocol']);
    assert.equal(request.protocol, 2);
    assert.equal(request.operation, 'test.list');
    assert.deepEqual(request.params, {});
    socket.end(`${JSON.stringify({ protocol: 2, id: request.id, ok: true, data: { runs: [] } })}\n`);
  }, (client) => client.call('test.list', {}, 'owner@example.test'));
  assert.deepEqual(response.data, { runs: [] });
});

test('daemon client rejects old, mismatched, extra, and unframed responses', async () => {
  for (const response of [
    (id) => `${JSON.stringify({ protocol: 1, id, ok: true, result: {} })}\n`,
    () => `${JSON.stringify({ protocol: 2, id: 'wrong', ok: true, data: {} })}\n`,
    (id) => `${JSON.stringify({ protocol: 2, id, ok: true, data: {}, extra: true })}\n`,
    (id) => JSON.stringify({ protocol: 2, id, ok: true, data: {} }),
  ]) {
    await assert.rejects(
      fixture((socket, raw) => {
        const request = JSON.parse(raw.subarray(0, -1));
        socket.end(response(request.id));
      }, (client) => client.call('ping')),
      /protocol-v2|newline-terminated/,
    );
  }
});

test('daemon client enforces request and response byte caps', async () => {
  await assert.rejects(
    fixture((socket) => socket.end(Buffer.alloc(256 * 1024 + 1, 0x20)),
      (client) => client.call('ping')),
    /response exceeds 256 KiB/,
  );
  await assert.rejects(
    fixture((socket) => socket.destroy(),
      (client) => client.call('test.start', { value: 'x'.repeat(64 * 1024) })),
    /request exceeds 64 KiB/,
  );
});
