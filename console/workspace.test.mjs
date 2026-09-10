import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

const context = vm.createContext({ window: {} });
vm.runInContext(await fs.readFile(new URL('./workspace.js', import.meta.url), 'utf8'), context);
const { catalogue, href } = context.window.DevCoordinatorWorkspace;

test('one repository includes verified clones without merging their record identities', () => {
  const source = { key: 'verified-hdlripper-origin', name: 'hdlripper' };
  const repositories = [{ repository_id: 'main', display_name: 'hdlripper' }, { repository_id: 'daily', display_name: 'workspace' }, { repository_id: 'windows', display_name: 'workspace' }];
  const runs = repositories.map((record, index) => ({ ...record, repository_source: source, worktree_path: ['/fixtures/app/main', '/fixtures/app/main/.local/daily/workspace', '/fixtures/app/windows/.local/daily/workspace'][index] }));
  const groups = catalogue(repositories, runs, []);
  assert.equal(groups.length, 1);
  assert.equal(groups[0].name, 'hdlripper');
  assert.equal(groups[0].repositoryId, 'main');
  assert.equal(groups[0].records.length, 3);
  assert.equal(groups[0].paths.length, 3);
  assert.deepEqual(new Set(groups[0].records.map((record) => record.repository_id)), new Set(['main', 'daily', 'windows']));
});

test('matching names and nested paths alone never combine repositories', () => {
  const groups = catalogue([
    { repository_id: 'one', display_name: 'workspace', root_path: '/source' },
    { repository_id: 'two', display_name: 'workspace', root_path: '/source/workspace' },
  ], [{ repository_id: 'two', display_name: 'workspace', repository_source: { key: 'other-origin', name: 'workspace' } }], []);
  assert.equal(groups.length, 2);
});

test('the catalogue includes repositories without tests or plans and preserves source identity', () => {
  const groups = catalogue([{ repository_id: 'plans', display_name: 'Plan only' }], [
    { repository_id: 'run', display_name: 'checkout', repository_source: { key: 'verified', name: 'Original' } },
  ], [{ repository_id: 'deployment', repository_name: 'Deployment only' }, { repository_id: 'run', repository_name: 'checkout' }]);
  assert.equal(groups.length, 3);
  assert.equal(groups.find((group) => group.repositoryId === 'run').name, 'Original');
});

test('collection routes carry repository context while ledger routes retain exact identity', () => {
  assert.equal(href('tests', 'repo/one'), '#/tests?repository=repo%2Fone');
  assert.equal(href('deployments', 'repo-one'), '#/deployments?repository=repo-one');
  for (const view of ['plan', 'progress', 'usage', 'decisions', 'glossary']) assert.equal(href(view, 'daily'), `#/${view}/daily`);
});
