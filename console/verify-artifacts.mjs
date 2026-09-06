import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';

const RUN = 't20260101T000100Z-caf123';
const EARLIER_RUN = 't20260101T000000Z-caf122';
const MANIFEST = 'a'.repeat(64);
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk/x8AAusB9Y9Z4rUAAAAASUVORK5CYII=', 'base64');
const FILES = new Map([
  ['capture.png', PNG],
  ['report.json', Buffer.from(JSON.stringify({ result: 'passed', hostile: '<img src=x onerror="window.artifactInjection=true">' }))],
  ['large.txt', Buffer.from('retained output\n'.repeat(80000))],
  ['unpreviewed.bin', Buffer.from([0, 1, 2, 255])],
  ...Array.from({ length: 99 }, (_, index) => [`nested/check-${index}.txt`, Buffer.from(`check ${index}\n`)]),
]);
const entries = [...FILES].map(([file, buffer]) => ({ path: file, size: buffer.length, sha256: crypto.createHash('sha256').update(buffer).digest('hex') }));
const artifact = { name: 'native-evidence', files: entries.length, size: entries.reduce((total, entry) => total + entry.size, 0), sha256: 'b'.repeat(64) };

export function artifactResponse(command, params, scenario) {
  if (!scenario.artifactFiles) return null;
  if (command === 'test.list') return { ok: true, data: { runs: [{
    run_id: RUN, test: 'native-editor', status: 'passed', display_name: 'Native editor',
    worktree_path: '/srv/repos/native-editor', worktree_id: 'native-worktree',
    repository_id: 'native-repository', started_at: '2026-01-01T00:01:00Z', duration_seconds: 4,
    checks: scenario.artifactCurrentEmpty ? [] : [{ name: 'native-session', status: 'passed', retained_artifacts: [artifact] }],
    visual_evidence: { status: 'unavailable', bundle_count: 0, image_count: 0, issue_count: 0 },
  }] } };
  if (command === 'test.history') {
    assert.equal(params.path, '/srv/repos/native-editor');
    assert.ok(params.limit <= 50);
    if (params.before) assert.equal(params.before, RUN);
    return { ok: true, data: {
      runs: [{ run_id: params.before ? EARLIER_RUN : RUN, test: params.before ? 'earlier-native-editor' : 'native-editor',
        status: 'passed', started_at: params.before ? '2026-01-01T00:00:00Z' : '2026-01-01T00:01:00Z', finished_at: null, duration_seconds: 4 }],
      next_before: params.before ? null : RUN,
    } };
  }
  if (command === 'test.log.catalog') {
    assert.equal(params.phase, undefined);
    assert.ok([RUN, EARLIER_RUN].includes(params.run_id));
    return { ok: true, data: {
      entries: [{ log_ref: { run_id: params.run_id, check: params.run_id === EARLIER_RUN ? 'earlier-session' : 'native-session', phase: 'check', stream: 'stdout' } }],
      next_cursor: null,
    } };
  }
  if (!command.startsWith('test.artifact.')) return null;
  if (scenario.artifactDenied) return { ok: false, error: { code: 'permission_denied', message: 'Administrator access required.', detail: '' } };
  if (scenario.artifactExpired) return { ok: false, error: { code: 'test_artifact_expired', message: 'Retained files have expired.', detail: '' } };
  assert.equal(params.path, '/srv/repos/native-editor');
  assert.ok([RUN, EARLIER_RUN].includes(params.run_id));
  assert.equal(params.check, params.run_id === EARLIER_RUN ? 'earlier-session' : 'native-session');
  if (params.artifact) assert.equal(params.artifact, artifact.name);
  if (command === 'test.artifact.catalog') {
    if (params.offset) assert.equal(params.manifest_sha256, MANIFEST);
    const offset = params.offset || 0;
    const end = Math.min(entries.length, offset + params.limit);
    return { ok: true, data: { run_id: params.run_id, check: params.check, manifest_sha256: MANIFEST, artifact, artifacts: [artifact], entries: entries.slice(offset, end), next_offset: end < entries.length ? end : null } };
  }
  assert.equal(command, 'test.artifact.file');
  assert.equal(params.manifest_sha256, MANIFEST);
  assert.ok(params.max_bytes > 0 && params.max_bytes <= 184320);
  const entry = entries.find((item) => item.path === params.file);
  const source = FILES.get(params.file);
  assert.ok(entry && source);
  const block = source.subarray(params.offset, params.offset + params.max_bytes);
  const next = params.offset + block.length;
  return { ok: true, data: { run_id: params.run_id, check: params.check, artifact: artifact.name, file: params.file,
    sha256: scenario.artifactChanged ? 'c'.repeat(64) : entry.sha256,
    total_bytes: entry.size, offset: params.offset, bytes: block.length, base64: block.toString('base64'), next_offset: next < entry.size ? next : null } };
}

export async function verifyTestArtifacts({ page, daemon, check, baseUrl, output, theme, viewport }) {
  const prefix = `files ${theme} ${viewport.width}`;
  const verify = (name, value, detail = '') => check(`${prefix}: ${name}`, value, detail);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  daemon.setScenario({ artifactFiles: true });
  await page.goto(`${baseUrl}#/tests`);
  await page.waitForSelector('.test-result-summary');
  await page.locator('.test-result-summary').click();
  verify('retained files are discoverable without a formal web bundle', await page.locator('[data-test-artifacts]').count() === 1);
  if (!await page.locator('[data-test-artifacts]').count()) return;
  verify('absence of a formal bundle is not absence of evidence', !/Evidence not produced|Evidence pending/.test(await page.locator('main').innerText()));
  await page.locator('[data-test-artifacts]').click();
  const dialog = page.locator('#test-artifacts-dialog');
  await dialog.locator('img').waitFor();
  await dialog.locator('img').evaluate((image) => image.decode());
  verify('image preview decodes', await dialog.locator('img').evaluate((image) => image.complete && image.naturalWidth === 1));
  verify('opening keeps continuation in view', await dialog.evaluate((element) => element.contains(document.activeElement) && element.getBoundingClientRect().top >= 0));
  await page.screenshot({ path: path.join(output, `files-${theme}-${viewport.width}.png`) });
  await dialog.getByRole('button', { name: 'report.json', exact: true }).click();
  await dialog.locator('pre').waitFor();
  verify('reports render as escaped text', (await dialog.locator('pre').innerText()).includes('<img') && !await page.evaluate(() => !!window.artifactInjection));
  const downloadEvent = page.waitForEvent('download');
  await dialog.getByRole('button', { name: 'Download file', exact: true }).click();
  const download = await downloadEvent;
  verify('download preserves original bytes', (await fs.readFile(await download.path())).equals(FILES.get('report.json')));
  await dialog.getByRole('button', { name: 'More files', exact: true }).click();
  await dialog.getByRole('button', { name: 'nested/check-98.txt', exact: true }).waitFor();
  verify('catalogue follows its bounded continuation', await dialog.locator('[data-artifact-file]').count() === entries.length);
  verify('pagination preserves the selected report', await dialog.locator('pre').count() === 1);
  await dialog.getByRole('button', { name: 'large.txt', exact: true }).click();
  await dialog.getByText('Preview limited to 1 MiB. Download the file for the complete content.', { exact: true }).waitFor();
  verify('large text is explicitly bounded', (await dialog.locator('pre').innerText()).length <= 1048576);
  await dialog.getByRole('button', { name: 'unpreviewed.bin', exact: true }).click();
  await dialog.getByText('No browser preview for this file type.', { exact: true }).waitFor();
  verify('binary is downloadable without rendering active content', await dialog.getByRole('button', { name: 'Download file', exact: true }).isEnabled());
  verify('dialog does not overflow the viewport', await dialog.evaluate((element) => element.getBoundingClientRect().right <= innerWidth && element.getBoundingClientRect().bottom <= innerHeight && element.scrollWidth <= element.clientWidth));
  await page.keyboard.press('Escape');
  verify('cancel restores focus', await page.locator('[data-test-artifacts]').evaluate((element) => element === document.activeElement));
  for (const [scenario, message] of [['artifactExpired', 'Retained files have expired.'], ['artifactDenied', 'Administrator access required.'], ['artifactChanged', 'The retained file changed or its response was incomplete.']]) {
    daemon.setScenario({ artifactFiles: true, [scenario]: true });
    await page.locator('[data-test-artifacts]').click();
    await dialog.getByText(message, { exact: true }).waitFor();
    verify(`${scenario} stays honest and does not show a stale image`, await dialog.locator('img').count() === 0);
    daemon.setScenario({ artifactFiles: true });
    await dialog.getByRole('button', { name: 'Try again', exact: true }).click();
    await dialog.locator('img').waitFor();
    await dialog.locator('img').evaluate((image) => image.decode());
    verify(`${scenario} recovers on exact retry`, await dialog.locator('img').evaluate((image) => image.naturalWidth === 1));
    await dialog.getByRole('button', { name: 'Close evidence files', exact: true }).click();
  }
  daemon.setScenario({ artifactFiles: true, artifactCurrentEmpty: true });
  await page.reload();
  await page.locator('.test-result-summary').click();
  await page.locator('[data-test-artifacts]').click();
  await dialog.getByLabel('Run', { exact: true }).selectOption(EARLIER_RUN);
  await dialog.locator('img').waitFor();
  await dialog.locator('img').evaluate((image) => image.decode());
  verify('newer nonvisual runs do not hide retained earlier files', await dialog.locator('img').evaluate((image) => image.naturalWidth === 1));
  verify('earlier evidence uses its own run and check', daemon.calls.some((call) => call.operation === 'test.artifact.file' && call.params.run_id === EARLIER_RUN && call.params.check === 'earlier-session'));
  verify('earlier evidence does not inherit the latest test label', /earlier-native-editor/.test(await dialog.locator('.artifact-run-facts').innerText()));
  await dialog.getByLabel('Run', { exact: true }).selectOption(RUN);
  await dialog.getByText('No retained files are available for this run.', { exact: true }).waitFor();
  verify('switching to a run without files clears old screenshots', await dialog.locator('img').count() === 0);
  await dialog.getByRole('button', { name: 'Close evidence files', exact: true }).click();
  verify('no browser errors', errors.length === 0, errors.join('; '));
}
