import path from 'node:path';

export async function verifyTestsDesign({ page, daemon, check, scenario, baseUrl, output, theme, viewport }) {
  const verify = (name, condition, detail = '') => check(`tests ${theme} ${viewport.width}: ${name}`, condition, detail);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  daemon.setScenario({ ...scenario, earlierEvidence: true });
  await page.goto(`${baseUrl}#/tests`);
  await page.waitForSelector('.test-repository');
  await page.locator('.test-repository').filter({ hasText: 'repo-one' }).click();
  verify('Tests belongs to the repository pane only', await page.locator('.tests-sidebar h1').innerText() === 'Tests' && await page.locator('#test-runs-collection>h1, #test-runs-collection>h2').count() === 0);
  verify('the selected repository is not repeated over results', !/repo-one|Latest results|Latest test runs|Repositories/.test(await page.locator('#test-runs-collection').innerText()));
  verify('results begin in the initial viewport', (await page.locator('.test-result-summary').first().boundingBox()).y < 200);
  verify('no overflow', await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  const grouping = await page.evaluate(() => {
    const source = { key: 'verified-origin', name: 'actual-project' };
    return window.DevCoordinatorTests.groupRuns([
      { run_id: 'older', repository_id: 'clone1', repository_source: source, display_name: 'uuid-folder', started_at: '2026-09-06T00:00:00Z' },
      { run_id: 'unrelated', repository_id: 'other', display_name: 'actual-project', started_at: '2026-09-05T00:00:00Z' },
      { run_id: 'newer', repository_id: 'clone2', repository_source: source, display_name: 'another-folder', started_at: '2026-09-07T00:00:00Z' },
    ]).map((group) => ({ name: group.name, runs: group.runs.map((run) => run.run_id) }));
  });
  verify('verified origins group clones, not unrelated names, and newest comes first', grouping.length === 2 && grouping[0].name === 'actual-project' && grouping[0].runs.join() === 'newer,older');
  await page.locator('.test-thumbnail').first().waitFor();
  verify('previews need no result disclosure', await page.locator('details.test-detail[open]').count() === 0 && await page.locator('.test-thumbnail').count() === 4);
  verify('earlier screenshots keep provenance', /Earlier run/.test(await page.locator('.test-preview-provenance').innerText()));
  const dimensions = await page.locator('.test-thumbnail img').first().boundingBox();
  verify('thumbnails stay small', dimensions.width <= 112 && dimensions.height <= 76);
  await page.locator('.test-thumbnail').nth(1).click();
  await page.locator('.test-image-preview[open] img').waitFor();
  verify('preview is focused and links to the exact image', await page.locator('.test-image-preview:focus-within').count() === 1 && (await page.locator('.test-image-preview a').getAttribute('href')).includes(`?image=${'2'.repeat(64)}`));
  await page.keyboard.press('Escape');
  verify('closing preview returns to the thumbnail', await page.locator('.test-thumbnail:focus').count() === 1 && await page.locator('.test-image-preview').count() === 0);
  await page.locator('.test-thumbnail').nth(1).click();
  daemon.calls.length = 0;
  await page.locator('.test-image-preview a').click();
  await page.waitForSelector('.evidence-workspace');
  verify('commenter opens the exact earlier run', daemon.calls.some((call) => call.operation === 'test.evidence.get' && call.params.run_id === 't20251231T000000Z-abc111' && call.params.path === '/srv/repos/repo-one'));
  verify('commenter selects the clicked screenshot', await page.locator('[data-evidence-viewport][aria-pressed=true]').getAttribute('data-evidence-viewport') === 'mobile');
  await page.goto(`${baseUrl}#/tests`);
  await page.waitForSelector('.test-repository');
  daemon.calls.length = 0;
  await page.locator('.test-repository').last().click();
  verify('repository switching is local', !daemon.calls.some((call) => call.operation === 'test.list') && await page.locator('.test-result').count() === 1);
  await page.reload();
  await page.waitForSelector('.test-repository[aria-pressed=true]');
  verify('repository selection survives reload', !/repo-one/.test(await page.locator('.test-repository[aria-pressed=true]').innerText()));
  await page.click('#test-run-open');
  await page.locator('#test-run-form [name=tier]').selectOption('development');
  const priorTime = await page.locator('.test-result time').getAttribute('datetime');
  await page.waitForFunction((before) => document.querySelector('.test-result time')?.getAttribute('datetime') !== before, priorTime);
  verify('background refresh retains inline choices and focus', await page.locator('#test-run-form [name=tier]').inputValue() === 'development' && await page.locator('#test-run-form:focus-within').count() === 1);
  await page.locator('#test-run-form [data-cancel]').click();
  daemon.setScenario({ ...scenario, denied: true });
  await page.getByRole('button', { name: 'Run again', exact: true }).click();
  await page.locator('.test-action-error').filter({ hasText: /requires/ }).waitFor();
  verify('denied reruns preserve the result and explain the failure inline', await page.locator('.test-result-summary>.badge').textContent() === 'failed' && await page.locator('dialog[open]').count() === 0);
  daemon.setScenario({ ...scenario, earlierEvidence: true });
  daemon.calls.length = 0;
  await page.getByRole('button', { name: 'Run again', exact: true }).click();
  await page.waitForFunction(() => !document.querySelector('[data-test-start]')?.disabled);
  verify('rerun acts directly on the exact checkout, named test and tier', daemon.calls.some((call) => call.operation === 'test.start' && call.params.path.includes('a-very-long-deployment-name') && call.params.test === 'ui-release' && call.params.tier === 'release') && await page.locator('#test-run-dialog').count() === 0);
  await page.locator('.test-repository').filter({ hasText: 'repo-one' }).click();
  await page.getByRole('button', { name: 'Logs', exact: true }).click();
  await page.waitForSelector('#test-logs-dialog[open] .test-log-scroll');
  verify('logs open immediately', await page.locator('#test-logs-dialog pre.log').count() > 0);
  await page.keyboard.press('Escape');
  await page.locator('.test-detail>summary').click();
  verify('technical metadata is optional', await page.locator('.test-technical').isVisible());
  daemon.setScenario({ ...scenario, earlierEvidence: true, testFinished: true });
  await page.waitForFunction(() => document.querySelector('.test-result-summary>.badge')?.textContent === 'passed');
  verify('live completion retains expanded details', await page.locator('.test-technical').isVisible());
  await page.click('#test-run-open');
  await page.waitForSelector('#test-run-form');
  verify('run choices appear in place without a dialog', await page.locator('#test-run-form:focus-within').count() === 1 && await page.locator('#test-run-form [name=tier]').inputValue() === 'release');
  await page.locator('#test-run-form [name=tier]').selectOption('pre-merge');
  daemon.calls.length = 0;
  await page.locator('#test-run-form button[type=submit]').click();
  await page.waitForSelector('#test-run-form', { state: 'detached' });
  verify('selected test and validation are submitted', daemon.calls.some((call) => call.operation === 'test.start' && call.params.test === 'unit' && call.params.path === '/srv/repos/repo-one' && call.params.tier === 'pre-merge'));
  await page.click('#test-run-open');
  await page.locator('#test-run-form [data-cancel]').click();
  verify('cancel restores run action focus', await page.locator('#test-run-open:focus').count() === 1);
  await revealTestSettings(page);
  await page.click('#test-capacity-open');
  await page.waitForSelector('#test-capacity-dialog[open]');
  await page.click('#test-capacity-cancel');
  await revealTestSettings(page);
  await page.click('#test-log-retention-open');
  await page.waitForSelector('#test-log-retention-dialog[open]');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  verify('settings dismiss with keyboard', await page.locator('.test-settings[open]').count() === 0);
  await page.screenshot({ path: path.join(output, `tests-pane-${theme}-${viewport.width}.png`), fullPage: true });
  await page.click('#theme-toggle');
  await page.reload();
  await page.waitForSelector('.test-results');
  verify('theme persists', await page.getAttribute('html', 'data-theme') !== theme);
  daemon.setScenario(scenario);
  await page.reload();
  await page.locator('.test-repository').filter({ hasText: 'repo-one' }).click();
  daemon.calls.length = 0;
  await page.getByRole('button', { name: 'Stop run', exact: true }).click();
  await page.waitForFunction(() => document.querySelector('.test-result-summary>.badge')?.textContent === 'cancelled');
  verify('stopping updates the exact run and removes its stop action', daemon.calls.some((call) => call.operation === 'test.stop' && call.params.path === '/srv/repos/repo-one') && await page.getByRole('button', { name: 'Stop run', exact: true }).count() === 0);
  daemon.setScenario({ ...scenario, empty: true });
  await page.reload();
  await page.getByText('No test runs yet.', { exact: true }).waitFor();
  verify('empty state is honest', await page.locator('#test-run-open').isDisabled());
  daemon.setScenario({ ...scenario, error: true });
  await page.reload();
  await page.waitForSelector('main .notice');
  verify('failed reads show no fake results', await page.locator('.test-result').count() === 0);
  daemon.setScenario(scenario);
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.waitForSelector('.test-result');
  daemon.setScenario({ ...scenario, denied: true, admin: false });
  await page.reload();
  await page.waitForSelector('main .notice.denied');
  verify('authorization still gates the collection', await page.locator('[data-test-start], #test-run-open').count() === 0);
  verify('no browser exceptions', errors.length === 0, errors.join('; '));
}

export async function revealTestRows(page) {
  await page.waitForSelector('.test-result-summary');
}

export async function revealTestSettings(page) {
  await page.waitForSelector('.test-settings>summary');
  if (await page.locator('.test-settings:not([open])').count()) await page.locator('.test-settings>summary').click();
}
