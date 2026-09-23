import fs from 'node:fs/promises';
import path from 'node:path';
import { formatMessage } from './i18n-core.mjs';

export async function verifyTranslatedConsole({ page, check, baseUrl, output, theme, viewport }) {
  const locale = process.env.CONSOLE_VERIFY_LOCALE || 'uk';
  const verify = (name, pass, detail = '') => check(`Translated ${locale} ${theme} ${viewport.width}: ${name}`, pass, detail);
  const manifest = JSON.parse(await fs.readFile(new URL('./locales/manifest.json', import.meta.url)));
  // A draft may be verified against its actual files before public admission.
  const entry = manifest.locales.find(entry => entry.tag === locale);
  if (!entry) throw new Error('Unknown test locale');
  entry.status = 'enabled';
  await page.route('**/locales/manifest.json', route => route.fulfill({ json: manifest }));
  const catalogs = {};
  for (const [ns, files] of Object.entries(entry.files)) {
    catalogs[ns] = Object.assign({}, ...await Promise.all(files.map(file => fs.readFile(new URL('./locales/' + file, import.meta.url), 'utf8').then(JSON.parse))));
  }
  await page.addInitScript(tag => localStorage.setItem('dc2-locale', tag), locale);
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  const repository = 'r0123456789abcdef';
  const routes = [`deployments?repository=${repository}`,`plan/${repository}`,`progress/${repository}`,`usage/${repository}`,`performance/${repository}`,`decisions/${repository}`,`tests?repository=${repository}`,'health','health/containers','bugs','admin',`sketches/${repository}`];
  for (const route of routes) {
    await page.goto(`${baseUrl}#/${route}`);
    await page.waitForFunction(() => !document.querySelector('.skeleton'));
    await page.locator('main h1, main .notice').first().waitFor();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    verify(`${route}: locale is active`, await page.locator('html').getAttribute('lang') === locale);
    const marked = await page.locator('[data-i18n]').evaluateAll(nodes => nodes.filter(node => node.getBoundingClientRect().width).map(node => ({ key: node.dataset.i18n, args: JSON.parse(node.dataset.i18nArgs || '{}'), value: node.textContent })));
    const wrong = [];
    for (const item of marked) {
      const dot = item.key.indexOf('.'); const pattern = catalogs[item.key.slice(0,dot)]?.[item.key.slice(dot+1)];
      if (pattern == null || item.value !== formatMessage(pattern,item.args,locale)) wrong.push(item.key);
    }
    verify(`${route}: rendered bindings use the real translated catalog`, wrong.length === 0, wrong.join(', '));
    const overflow = await page.evaluate(() => ({ width: document.documentElement.scrollWidth - innerWidth, elements: [...document.querySelectorAll('body *')].map(element => ({ tag: element.tagName, cls: element.className, right: element.getBoundingClientRect().right, left: element.getBoundingClientRect().left })).filter(item => item.right > innerWidth + 1 || item.left < -1).slice(0, 8) }));
    verify(`${route}: no document overflow`, overflow.width <= 1, JSON.stringify(overflow));
  }
  await page.goto(`${baseUrl}#/plan/${repository}`);
  await page.waitForFunction(() => !document.querySelector('.skeleton'));
  await page.locator('#language-toggle').click();
  const row = page.locator(`.language-option[data-locale="${locale}"]`).first();
  verify('native name retained', await row.locator('.language-names > span').nth(1).innerText() === entry.nativeName);
  verify('localized name and native name both present', await row.locator('.language-names > span').count() === 2);
  await page.screenshot({ path: path.join(output, `translated-${locale}-${theme}-${viewport.width}.png`), mask: [page.locator('#who-email')] });
  verify('no page errors across routes', errors.length === 0, errors.join('; '));
}
