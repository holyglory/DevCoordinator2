// Route-document store: reads the daemon's atomic snapshot, validates it,
// keeps the last valid document across daemon restarts, never clears served
// routes because of a malformed or partial file. (docs/route-document.md)

import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';

const MAX_BYTES = 2 * 1024 * 1024;
const POLL_MS = 5000;

export function validateDocument(text) {
  if (typeof text !== 'string' || text.length === 0 || text.length > MAX_BYTES) {
    throw new Error('route document is empty or oversized');
  }
  const doc = JSON.parse(text);
  if (doc?.schema !== 1) throw new Error('unsupported route schema');
  const { schema: _s, payload_sha256: sha, ...payload } = doc;
  const canonical = canonicalJson(payload);
  const expected = crypto.createHash('sha256').update(canonical).digest('hex');
  if (sha !== expected) throw new Error('route document checksum mismatch');
  if (!Number.isInteger(doc.generation) || !Array.isArray(doc.routes)) {
    throw new Error('route document missing generation or routes');
  }
  for (const r of doc.routes) {
    if (typeof r.domain !== 'string' || !Number.isInteger(r.port) || typeof r.deployment_id !== 'string') {
      throw new Error('route entry malformed');
    }
  }
  if (!doc.access || !Array.isArray(doc.access.owners) || !Array.isArray(doc.access.grants)) {
    throw new Error('route document missing access section');
  }
  return doc;
}

// Python json.dumps(sort_keys=True, separators=(",", ":")) equivalence for the
// payload produced by routes.py (strings, ints, floats, bools, null, arrays, objects).
export function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  const keys = Object.keys(value).sort();
  return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(value[k])}`).join(',')}}`;
}

export async function createRoutesStore({ file, stateDir, log }) {
  const lkgFile = path.join(stateDir, 'routes.last-known-good.json');
  let current = { schema: 1, generation: 0, routes: [], access: { owners: [], grants: [] }, domain: '' };
  let source = 'none';

  async function tryLoad(candidate, label) {
    let text;
    try {
      text = await fsp.readFile(candidate, 'utf8');
    } catch {
      return false;
    }
    let doc;
    try {
      doc = validateDocument(text);
    } catch (error) {
      log?.warn?.('route document rejected', { file: candidate, error: error.message });
      return false;
    }
    if (doc.generation < current.generation) {
      log?.warn?.('route document older than served generation ignored', { generation: doc.generation, served: current.generation });
      return false;
    }
    if (doc.generation === current.generation && source !== 'none') return true;
    current = doc;
    source = label;
    if (label === 'live') {
      await fsp.mkdir(stateDir, { recursive: true });
      const tmp = `${lkgFile}.tmp`;
      await fsp.writeFile(tmp, text, { mode: 0o600 });
      await fsp.rename(tmp, lkgFile);
    }
    log?.info?.('route document loaded', { generation: doc.generation, routes: doc.routes.length, source: label });
    return true;
  }

  await fsp.mkdir(stateDir, { recursive: true });
  await tryLoad(lkgFile, 'last-known-good');
  await tryLoad(file, 'live');

  let timer = setInterval(() => { tryLoad(file, 'live').catch(() => {}); }, POLL_MS);
  timer.unref();
  let watcher = null;
  try {
    watcher = fs.watch(path.dirname(file), () => { tryLoad(file, 'live').catch(() => {}); });
    watcher.unref?.();
  } catch {
    watcher = null;
  }

  return {
    current: () => current,
    source: () => source,
    reload: () => tryLoad(file, 'live'),
    close: () => { clearInterval(timer); watcher?.close(); },
  };
}
