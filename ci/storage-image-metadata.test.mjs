import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { promisify } from 'node:util';

const execute = promisify(execFile);

test('Docker renders complete image metadata with absent ownership kept unknown', async (t) => {
  const source = await readFile(new URL('../rust/control/src/storage/native/docker.rs', import.meta.url), 'utf8');
  const format = source.match(/^const IMAGE_FORMAT: &str = r#"(.+)"#;$/m)?.[1];
  assert.ok(format, 'use the production observation template');
  const ownership = {
    'devcoordinator2.repository': 'r1111111111111111',
    'devcoordinator2.deployment': 'deployment-"quoted"\\value',
    'com.docker.compose.project': 'fixture project\nsecond line',
  };
  const configurations = [
    undefined,
    null,
    {},
    { User: 'fixture' },
    { Labels: null },
    { Labels: {} },
    { Labels: { 'org.opencontainers.image.title': 'Unrelated metadata' } },
    { Labels: ownership },
    { Labels: { 'devcoordinator2.repository': ownership['devcoordinator2.repository'] } },
  ];
  const images = configurations.map((Config, index) => ({
    Id: `sha256:${(index + 1).toString(16).padStart(64, '0')}`,
    Created: '2026-11-23T00:00:00Z',
    Size: 1234 + index,
    RepoTags: index % 2 ? null : [`fixture-${index}:metadata`],
    RepoDigests: [`fixture-${index}@sha256:${'a'.repeat(64)}`],
    ...(Config === undefined ? {} : { Config }),
  }));
  const directory = await mkdtemp(path.join(tmpdir(), 'dc2-image-'));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const socket = path.join(directory, 'engine.sock');
  const requests = [];
  const server = http.createServer((request, response) => {
    const route = decodeURIComponent(new URL(request.url, 'http://fixture.invalid').pathname);
    requests.push(`${request.method} ${route}`);
    response.setHeader('Connection', 'close');
    response.setHeader('API-Version', '1.41');
    if (route === '/_ping') {
      response.end('OK');
      return;
    }
    const id = route.match(/^\/v1\.41\/images\/(.+)\/json$/)?.[1];
    const image = images.find((value) => value.Id === id);
    response.setHeader('Content-Type', 'application/json');
    response.statusCode = image ? 200 : 404;
    response.end(JSON.stringify(image ?? { message: 'No such fixture image' }));
  });
  server.listen(socket);
  await once(server, 'listening');
  t.after(() => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())));
  const env = { ...process.env, DOCKER_CONFIG: path.join(directory, 'config'), DOCKER_API_VERSION: '1.41' };
  delete env.DOCKER_CONTEXT;
  delete env.DOCKER_TLS_VERIFY;
  delete env.DOCKER_CERT_PATH;
  const inspect = (...ids) => execute('docker', ['--host', `unix://${socket}`, 'image', 'inspect', '--format', format, ...ids], {
    env,
    timeout: 10_000,
    maxBuffer: 64 * 1024,
  });
  const { stdout, stderr } = await inspect(...images.map((image) => image.Id));
  assert.equal(stderr, '');
  assert.deepEqual(stdout.trim().split('\n').map((line) => JSON.parse(line)), images.map((image) => ({
    id: image.Id,
    created: image.Created,
    size: image.Size,
    tags: image.RepoTags,
    digests: image.RepoDigests,
    repository: image.Config?.Labels?.['devcoordinator2.repository'] ?? null,
    deployment: image.Config?.Labels?.['devcoordinator2.deployment'] ?? null,
    project: image.Config?.Labels?.['com.docker.compose.project'] ?? null,
  })));
  await assert.rejects(inspect(`sha256:${'f'.repeat(64)}`), (error) => error.code === 1);
  assert.equal(requests.filter((request) => request.includes('/images/')).length, images.length + 1);
});
