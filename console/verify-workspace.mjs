import path from 'node:path';
import { chooseRepository } from './verify-tests-pane.mjs';

export async function verifyWorkspace({ page, daemon, check, scenario, baseUrl, output, theme, viewport }) {
  const repositoryId = 'r0123456789abcdef';
  const verify = (name, condition, detail = '') => check(`workspace ${theme} ${viewport.width}: ${name}`, condition, detail);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  daemon.setScenario({ ...scenario, workspaceLongList: true });
  await page.goto(`${baseUrl}#/plan/${repositoryId}`);
  await page.waitForSelector('.plan-context');
  verify('direct plan links select the same repository', await page.locator('#workspace-heading').innerText() === 'repo-one');
  verify('one shared selector replaces page-specific pickers', await page.locator('#repository-list').count() === 1 && await page.locator('[data-project-picker], .test-repository-list').count() === 0);
  verify('all aspects have real repository-scoped links', await page.locator('#workspace-aspects a').count() === 5 && await page.locator('#workspace-aspects a[href="#/tests?repository=r0123456789abcdef"]').count() === 1);
  verify('plan and progress are one destination with three views', await page.locator('#workspace-work-views a').allTextContents().then((labels) => labels.join() === 'Plan,Progress,Usage'));
  const compressedTitles = await page.locator('.plan-task-title').evaluateAll((elements) => elements.filter((element) => {
    const lineHeight = parseFloat(getComputedStyle(element).lineHeight);
    return element.getBoundingClientRect().height + 1 < Math.min(element.scrollHeight, lineHeight * 2);
  }).length);
  verify('plan metadata does not compress task titles into partial lines', compressedTitles === 0);
  const sidebar = await page.locator('#repository-sidebar').elementHandle();
  for (const [view, selector, operation] of [
    ['progress', '.progress-workspace', 'progress.repository'],
    ['usage', '[data-ui-region="usage-primary-trend"]', 'usage.repository'],
    ['plan', '.plan-context', 'plan.overview'],
  ]) {
    daemon.calls.length = 0;
    await page.locator(`#workspace-work-views a[href="#/${view}/${repositoryId}"]`).click();
    await page.locator(selector).waitFor();
    verify(`${view} keeps the selected repository and queries its records`, daemon.calls.some((call) => call.operation === operation && call.params.repository_id === repositoryId) && await page.locator('#workspace-heading').innerText() === 'repo-one');
    const overflow = await page.evaluate(() => ({ width: document.documentElement.scrollWidth, viewport: innerWidth, elements: [...document.querySelectorAll('main > *, .plan-selection > *, .plan-context > *')].map((element) => ({ selector: element.className, right: element.getBoundingClientRect().right })).filter((element) => element.right > innerWidth + 1) }));
    verify(`${view} fits the viewport`, overflow.width <= overflow.viewport, JSON.stringify(overflow));
    if (overflow.width > overflow.viewport) await page.screenshot({ path: path.join(output, `${view}-overflow-${viewport.width}-${theme}.png`) });
  }
  await page.locator('#workspace-aspects a').filter({ hasText: 'Deployments' }).click();
  await page.locator('.deployment-record').first().waitFor();
  verify('deployments are scoped, without duplicate cross-aspect summaries', await page.locator('.deployment-record').count() === 2 && await page.locator('.deployment-repository-summary').count() === 0);
  verify('domains stay beside their deployments', await page.locator('.deployment-domain').count() === 2 && await page.locator('[data-edit-domain]').count() === 2);
  verify('deployment cards fit the selected-repository pane', await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
  verify('switching aspects keeps the original sidebar element', await sidebar.evaluate((element) => element.isConnected));
  await page.locator('[data-edit-domain]').first().click();
  await page.locator('#domain-dialog[open]').waitFor();
  await page.keyboard.press('Escape');
  verify('canceling a domain edit preserves the repository', await page.locator('#workspace-heading').innerText() === 'repo-one');
  await page.locator('.deployment-record-title a').first().click();
  await page.locator('[data-cmd="deployment.restart"]').first().waitFor();
  verify('deployment detail remains inside the workspace', await page.locator('#workspace-heading').innerText() === 'repo-one');
  await page.goBack();
  await page.locator('.deployment-record').first().waitFor();
  await page.locator('#workspace-aspects a').filter({ hasText: 'Decisions' }).click();
  await page.locator('#decision-search').waitFor();
  verify('the summary does not displace decision search', !await page.locator('.story').getAttribute('open') && (await page.locator('#decision-search').boundingBox()).y < 500);
  await page.locator('.story summary').click();
  verify('the decision summary opens in context', await page.locator('.story[open]').count() === 1);
  await page.locator('.story summary').click();
  await page.locator('#decision-search input').fill('export');
  await page.locator('#decision-search button[type="submit"]').click();
  await page.waitForFunction(() => document.querySelector('main')?.textContent.includes('Exports are downloadable files'));
  verify('decisions search uses the current repository', daemon.calls.some((call) => call.operation === 'decision.search' && call.params.repository_id === repositoryId));
  await chooseRepository(page, 'a-very-long-deployment-name');
  await page.waitForFunction(() => location.hash === '#/decisions/r2');
  await page.locator('#decision-search').waitFor();
  verify('switching repository preserves the current aspect', (await page.locator('#workspace-heading').innerText()).startsWith('a-very-long'));
  await page.reload();
  await page.locator('#decision-search').waitFor();
  verify('reload retains repository and aspect', (await page.locator('#workspace-heading').innerText()).startsWith('a-very-long') && await page.locator('#workspace-aspects a[aria-current]').innerText() === 'Decisions');
  await page.goBack();
  await page.locator('#decision-search').waitFor();
  verify('Back restores the prior repository', await page.locator('#workspace-heading').innerText() === 'repo-one');
  await page.locator('#workspace-aspects a').filter({ hasText: 'Tests' }).click();
  await page.locator('.test-result').first().waitFor();
  if (viewport.width <= 760) {
    verify('narrow layout puts results before the repository drawer', !await page.locator('#repository-sidebar').isVisible() && (await page.locator('.test-result').first().boundingBox()).y < 330);
    await page.click('#repository-toggle');
    verify('repository drawer focuses search', await page.locator('#repository-search:focus').count() === 1);
    await page.keyboard.press('Escape');
    verify('Escape returns to the repository trigger', await page.locator('#repository-toggle:focus').count() === 1 && !await page.locator('#repository-sidebar').isVisible());
    await page.click('#repository-toggle');
  }
  const overlappingLabels = await page.locator('#repository-list a').evaluateAll((elements) => elements.filter((element) => {
    const row = element.getBoundingClientRect();
    const text = element.querySelector('span').getBoundingClientRect();
    return text.top < row.top - 1 || text.bottom > row.bottom + 1;
  }).length);
  verify('long duplicate checkout paths stay inside their own repository rows', overlappingLabels === 0);
  await page.locator('#repository-search').fill('no-such-repository');
  verify('search has a truthful empty state', await page.locator('#repository-list a').count() === 0 && /No matching/.test(await page.locator('#repository-list').innerText()));
  await page.locator('#repository-search').fill('repo-one');
  verify('search filters the shared list', await page.locator('#repository-list a').count() === 1);
  await page.locator('#repository-search').fill('');
  if (viewport.width <= 760) await page.click('#repository-close');
  await page.click('#nav-toggle');
  await page.locator('#nav a[data-view="health"]').click();
  await page.locator('.health-summary').waitFor();
  verify('host tools do not pretend to be repository-scoped', !await page.locator('#workspace-context').isVisible());
  await page.click('#nav-toggle');
  await page.locator('#nav a[data-view="plan"]').click();
  await page.locator('.plan-context').waitFor();
  verify('returning from host tools remembers the repository', await page.locator('#workspace-heading').innerText() === 'repo-one');
  await page.goto(`${baseUrl}#/glossary/${repositoryId}`);
  await page.locator('#workspace-aspects a[aria-current]').waitFor();
  await page.evaluate(() => new Promise(requestAnimationFrame));
  const selectedAspect = await page.locator('#workspace-aspects a[aria-current]').boundingBox();
  const aspectStrip = await page.locator('#workspace-aspects').boundingBox();
  verify('direct links reveal the entire selected aspect on narrow screens', selectedAspect.x >= aspectStrip.x - 1 && selectedAspect.x + selectedAspect.width <= aspectStrip.x + aspectStrip.width + 1);
  await page.goto(`${baseUrl}#/plan/${repositoryId}`);
  await page.locator('.plan-context').waitFor();
  await page.screenshot({ path: path.join(output, `workspace-${theme}-${viewport.width}.png`), fullPage: true });
  verify('no uncaught browser errors', errors.length === 0, errors.join('; '));
}
