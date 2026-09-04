// One request, one response over the daemon's Unix socket (docs/protocol.md).
// The edge asserts the signed-in identity; the daemon trusts it only because
// the edge's peer uid is the configured edge uid.

import net from 'node:net';
import crypto from 'node:crypto';

export function createDaemonClient({ socketPath, timeoutMs = 1800000 }) {
  function call(command, args = {}, identity = null) {
    return new Promise((resolve, reject) => {
      const socket = net.createConnection(socketPath);
      const chunks = [];
      let settled = false;
      const finish = (fn, value) => { if (!settled) { settled = true; fn(value); } };
      socket.setTimeout(timeoutMs, () => { socket.destroy(); finish(reject, new Error('daemon timeout')); });
      socket.on('error', (error) => finish(reject, error));
      socket.on('connect', () => {
        const request = {
          protocol: 1,
          id: crypto.randomBytes(6).toString('hex'),
          command,
          args,
          client: { kind: 'edge', identity: identity || undefined },
        };
        socket.end(`${JSON.stringify(request)}\n`);
      });
      socket.on('data', (chunk) => chunks.push(chunk));
      socket.on('close', () => {
        const text = Buffer.concat(chunks).toString('utf8').trim();
        if (!text) return finish(reject, new Error('empty daemon response'));
        try {
          finish(resolve, JSON.parse(text));
        } catch (error) {
          finish(reject, error);
        }
      });
    });
  }
  return { call };
}
