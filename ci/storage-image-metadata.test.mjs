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

async function renderSiblingFixture(kind, objects) {
  const source = await readFile(new URL('../rust/control/src/storage/native/docker.rs', import.meta.url), 'utf8');
  const format = source.match(new RegExp(`^const ${kind.toUpperCase()}_FORMAT: &str = r#"(.+)"#;$`, 'm'))?.[1];
  assert.ok(format, 'use the production sibling observation template');
  const directory = await mkdtemp(path.join(tmpdir(), `dc2-${kind}-`));
  const socket = path.join(directory, 'engine.sock');
  const routes = {
    container: /^\/v1\.41\/containers\/(.+)\/json$/,
    network: /^\/v1\.41\/networks\/(.+)$/,
    volume: /^\/v1\.41\/volumes\/(.+)$/,
  };
  let inspections = 0;
  const identity = (value) => kind === 'volume' ? value.Name : value.Id;
  const server = http.createServer((request, response) => {
    const route = decodeURIComponent(new URL(request.url, 'http://fixture.invalid').pathname);
    response.setHeader('Connection', 'close');
    response.setHeader('API-Version', '1.41');
    if (route === '/_ping') { response.end('OK'); return; }
    inspections++;
    const id = route.match(routes[kind])?.[1];
    const object = objects.find((value) => identity(value) === id);
    response.setHeader('Content-Type', 'application/json');
    response.statusCode = object ? 200 : 404;
    response.end(JSON.stringify(object ?? { message: 'No such fixture object' }));
  });
  server.listen(socket);
  await once(server, 'listening');
  try {
    const env = { ...process.env, DOCKER_CONFIG: path.join(directory, 'config'), DOCKER_API_VERSION: '1.41' };
    delete env.DOCKER_CONTEXT;
    delete env.DOCKER_TLS_VERIFY;
    delete env.DOCKER_CERT_PATH;
    const inspect = (...ids) => execute('/usr/bin/docker', ['--host', `unix://${socket}`, kind, 'inspect', '--format', format, ...ids], { env, timeout: 10_000, maxBuffer: 64 * 1024 });
    const { stdout, stderr } = await inspect(...objects.map(identity));
    assert.equal(stderr, '');
    const rendered = stdout.trim().split('\n').map((line) => JSON.parse(line));
    await assert.rejects(inspect('missing-fixture-object'), (error) => error.code === 1);
    assert.equal(inspections, objects.length + 1);
    return rendered;
  } finally {
    await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    await rm(directory, { recursive: true, force: true });
  }
}

test('container labels stay unknown while IDs, state and consumer mounts remain exact', async () => {
  const ownership = {
    'devcoordinator2.instance': 'fixture-instance',
    'devcoordinator2.repository': 'r1111111111111111',
    'devcoordinator2.deployment': 'deployment-"quoted"\\value',
    'com.docker.compose.project': 'fixture project\nsecond line',
  };
  const configurations = [undefined, null, {}, { User: 'fixture' }, { Labels: null }, { Labels: {} }, { Labels: { unrelated: 'metadata' } }, { Labels: ownership }, { Labels: { 'devcoordinator2.repository': ownership['devcoordinator2.repository'] } }];
  const containers = configurations.map((Config, index) => ({
    Id: (index + 4096).toString(16).padStart(64, '0'),
    Name: `/fixture-${index}`,
    Created: '2026-11-23T00:00:00Z',
    State: { Status: index % 2 ? 'created' : 'exited', FinishedAt: '2026-11-23T00:01:00Z' },
    Image: `sha256:${'b'.repeat(64)}`,
    Mounts: [{ Type: 'volume', Name: `fixture-volume-${index}`, Source: `/fixture/${index}/_data` }, { Type: 'bind', Source: `/fixture/${index}/bind` }],
    ...(Config === undefined ? {} : { Config }),
  }));
  assert.deepEqual(await renderSiblingFixture('container', containers), containers.map((container) => ({
    id: container.Id,
    name: container.Name,
    created: container.Created,
    state: container.State.Status,
    finished: container.State.FinishedAt,
    image: container.Image,
    instance: container.Config?.Labels?.['devcoordinator2.instance'] ?? null,
    repository: container.Config?.Labels?.['devcoordinator2.repository'] ?? null,
    deployment: container.Config?.Labels?.['devcoordinator2.deployment'] ?? null,
    project: container.Config?.Labels?.['com.docker.compose.project'] ?? null,
    mounts: container.Mounts.map((mount) => ({ type: mount.Type, name: mount.Name ?? null, source: mount.Source })),
  })));
});

test('network labels stay unknown while IDs and all consumer references remain exact', async () => {
  const labels = [undefined, null, {}, { unrelated: 'metadata' }, { 'com.docker.compose.project': 'project-"quoted"\\value\nnext line' }];
  const networks = labels.map((Labels, index) => ({
    Id: (index + 8192).toString(16).padStart(64, '0'),
    Name: `fixture-network-${index}`,
    Created: '2026-11-23T00:00:00Z',
    Driver: 'bridge',
    Scope: 'local',
    Containers: { ['b'.repeat(64)]: { Name: 'fixture-consumer' } },
    ...(Labels === undefined ? {} : { Labels }),
  }));
  assert.deepEqual(await renderSiblingFixture('network', networks), networks.map((network) => ({
    id: network.Id,
    name: network.Name,
    created: network.Created,
    driver: network.Driver,
    project: network.Labels?.['com.docker.compose.project'] ?? null,
    containers: Object.keys(network.Containers),
  })));
});

test('volume null and empty labels keep the existing typed-map rendering', async () => {
  const labels = [null, {}, { 'devcoordinator2.repository': 'r1111111111111111', 'devcoordinator2.deployment': 'fixture-deployment', 'com.docker.compose.project': 'fixture-project' }];
  const volumes = labels.map((Labels, index) => ({ Name: `fixture-volume-${index}`, CreatedAt: '2026-11-23T00:00:00Z', Driver: 'local', Mountpoint: `/fixture/${index}/_data`, Options: null, Labels, Scope: 'local' }));
  assert.deepEqual(await renderSiblingFixture('volume', volumes), volumes.map((volume) => ({
    name: volume.Name,
    created: volume.CreatedAt,
    driver: volume.Driver,
    mountpoint: volume.Mountpoint,
    options: 0,
    repository: volume.Labels?.['devcoordinator2.repository'] ?? '',
    deployment: volume.Labels?.['devcoordinator2.deployment'] ?? '',
    project: volume.Labels?.['com.docker.compose.project'] ?? '',
  })));
});
