import path from 'node:path';

export async function verifyTestsDesign({ page, daemon, check, scenario, baseUrl, output, theme, viewport }) {
  const prefix = `${theme} ${viewport.width}`;
  const assert = (name, condition, detail = '') => check(`${prefix}: ${name}`, condition, detail);
  const failures = [];
  page.on('pageerror', (error) => failures.push(error.message));
  daemon.setScenario({ ...scenario, earlierEvidence: true });
  await page.goto(`${baseUrl}#/tests`);
  await page.waitForSelector('.test-result-summary');
  await page.evaluate(() => document.fonts.ready);
  assert('theme follows preference', await page.getAttribute('html', 'data-theme') === theme);
  assert('one unified result collection', await page.locator('.test-results').count() === 1 && !/needs attention/i.test(await page.locator('main').innerText()));
  assert('technical detail hidden initially', !await page.locator('.test-technical').first().isVisible());
  assert('list in first viewport', (await page.locator('.test-result-summary').first().boundingBox()).y < viewport.height / 2);
  await page.locator('.test-result-summary').first().click();
  const first = page.locator('.test-result').first();
  await first.locator('.test-evidence-popover>summary').click();
  assert('earlier evidence has provenance', /Earlier run/.test(await first.innerText()) && /do not verify the latest/.test(await first.innerText()));
  await page.screenshot({ path: path.join(output, `${prefix.replace(' ', '-')}-evidence.png`), fullPage: true });
  await page.keyboard.press('Escape');
  assert('escape dismisses evidence and restores focus', await first.locator('.test-evidence-popover[open]').count() === 0 && await first.locator('.test-evidence-popover>summary:focus').count() === 1);
  await first.locator('.test-evidence-popover>summary').click();
  daemon.calls.length = 0;
  await first.getByRole('link', { name: 'Open screenshots' }).click();
  await page.waitForSelector('.evidence-workspace');
  assert('earlier screenshot opens exact owned run', daemon.calls.some((call) => call.operation === 'test.evidence.get' && call.params.run_id === 't20251231T000000Z-abc111' && call.params.path === '/srv/repos/repo-one'));
  await page.goto(`${baseUrl}#/tests`);
  await page.locator('.test-result-summary').first().click();
  await first.getByRole('button', { name: 'Open logs' }).click();
  await page.waitForSelector('dialog[open] .test-log-scroll');
  assert('logs load in dialog', await page.locator('dialog[open]').isVisible());
  await page.keyboard.press('Escape');
  await first.locator('.test-detail>summary').last().click();
  assert('technical details revealed on demand', await first.locator('.test-technical').isVisible());
  assert('revealed output sizes are humanized', /MiB/.test(await first.locator('.test-technical').innerText()));
  daemon.setScenario({ ...scenario, testFinished: true, earlierEvidence: true });
  await page.waitForFunction(() => document.querySelector('.test-result-summary .badge')?.textContent === 'passed');
  assert('refresh preserves expanded context', await first.getAttribute('open') !== null && await first.locator('.test-technical').isVisible());
  assert('duration rounded and obsolete stop removed', !/3600\.\d/.test(await first.innerText()) && await first.locator('[data-cmd="test.stop"]').count() === 0);
  await first.getByRole('button', { name: 'Run again' }).click();
  await page.waitForSelector('#test-run-dialog[open]');
  assert('rerun opens focused editor', await page.locator('#test-run-dialog:focus-within').count() === 1);
  assert('release is rerun default', await page.locator('#test-run-dialog [name=tier]').inputValue() === 'release');
  await page.selectOption('#test-run-dialog [name=tier]', 'pre-merge');
  daemon.calls.length = 0;
  await page.locator('#test-run-dialog button[type=submit]').click();
  await page.waitForSelector('#test-run-dialog', { state: 'detached' });
  assert('rerun submits exact repository and tier', daemon.calls.some((call) => call.operation === 'test.start' && call.params.path === '/srv/repos/repo-one' && call.params.tier === 'pre-merge'));
  await page.click('#test-run-open');
  await page.locator('#test-run-dialog [data-cancel]').click();
  assert('cancel restores primary action focus', await page.locator('#test-run-open:focus').count() === 1);
  await page.locator('.test-settings>summary').click();
  await page.click('#test-capacity-open');
  await page.waitForSelector('#test-capacity-dialog[open]');
  await page.click('#test-capacity-cancel');
  assert('capacity cancellation preserves settings context', await page.locator('#test-capacity-open:focus').count() === 1);
  await page.click('#test-log-retention-open');
  await page.waitForSelector('#test-log-retention-dialog[open]');
  await page.keyboard.press('Escape');
  await page.keyboard.press('Escape');
  assert('settings keyboard dismissal', await page.locator('.test-settings[open]').count() === 0);
  await page.click('#theme-toggle');
  await page.reload();
  await page.waitForSelector('.test-results');
  assert('theme selection persists after reload', await page.getAttribute('html', 'data-theme') !== theme);
  await page.click('#theme-toggle');
  for (const route of ['tests', 'deployments', 'plan', 'progress', 'usage', 'decisions', 'health', 'bugs', 'admin']) {
    await page.goto(`${baseUrl}#/${route}`);
    await page.waitForFunction(() => !document.querySelector('main>.skeleton') && document.querySelector('main h1'));
    await page.evaluate(() => document.fonts.ready);
    assert(`${route} retains shared typography`, await page.locator('body').evaluate((element) => getComputedStyle(element).fontFamily.includes('Inter')));
    assert(`${route} fits viewport`, await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), await page.evaluate(() => String(document.documentElement.scrollWidth - innerWidth)));
    await page.screenshot({ path: path.join(output, `${prefix.replace(' ', '-')}-${route}.png`), fullPage: true });
  }
  daemon.setScenario({ ...scenario, empty: true });
  await page.goto(`${baseUrl}#/tests`);
  await page.getByText('No test runs yet.', { exact: true }).waitFor();
  assert('empty collection is honest', await page.locator('#test-run-open').isDisabled());
  daemon.setScenario({ ...scenario, error: true });
  await page.reload();
  await page.waitForSelector('main .notice');
  assert('failed data load is not a fabricated collection', await page.locator('.test-result').count() === 0);
  daemon.setScenario(scenario);
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.waitForSelector('.test-result-summary');
  assert('retry recovers the real collection', await page.locator('.test-result').count() === 2);
  daemon.setScenario({ ...scenario, denied: true, admin: false });
  await page.reload();
  await page.waitForSelector('main .notice.denied');
  assert('denied view does not expose run actions', await page.locator('[data-test-start], #test-run-open').count() === 0);
  assert('no browser exceptions', failures.length === 0, failures.join('; '));
}

export async function revealTestRows(page) {
  await page.waitForSelector('.test-result-summary');
  for (const summary of await page.locator('.test-result>.test-result-summary').all()) {
    if (!await summary.evaluate((element) => element.parentElement.open)) await summary.click();
  }
}

export async function revealTestSettings(page) {
  await page.waitForSelector('.test-settings>summary');
  if (await page.locator('.test-settings:not([open])').count()) await page.locator('.test-settings>summary').click();
}
