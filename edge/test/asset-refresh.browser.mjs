import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { createStaticServer } from '../lib/static.mjs';

const moduleRoot = process.env.CONSOLE_VERIFY_PLAYWRIGHT || path.resolve('ci/playwright');
const { chromium } = createRequire(path.join(moduleRoot, 'package.json'))('playwright');

test('ordinary browser reload replaces assets and manifest-mapped auth messages', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'dc2-browser-refresh-'));
  const assets = createStaticServer({ dir: directory });
  let measured = 41;
  const server = http.createServer((request, response) => {
    if (request.url === '/measurement') {
      response.writeHead(200, { 'content-type': 'application/json', 'cache-control': 'no-store' });
      response.end(JSON.stringify({ measured }));
    } else {
      assets.handle(request, response);
    }
  });
  let browser;
  try {
    await fs.writeFile(path.join(directory, 'index.html'), '<!doctype html><meta charset="utf-8"><title>Release fixture</title><link rel="stylesheet" href="/app.css"><main id="status"></main><script src="/app.js"></script>');
    await assets.prepare();
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    browser = await chromium.launch({ headless: true });
    for (const width of [1440, 390]) {
      await fs.writeFile(path.join(directory, 'app.js'), 'document.querySelector("#status").textContent = "Removed stale status";');
      await fs.writeFile(path.join(directory, 'app.css'), '#status { color: rgb(128, 0, 0); }');
      const context = await browser.newContext({ viewport: { width, height: 900 } });
      const page = await context.newPage();
      const errors = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      await page.waitForFunction(() => document.querySelector('#status').textContent === 'Removed stale status');
      measured += 1;
      await fs.writeFile(path.join(directory, 'app.js'), 'fetch("/measurement").then(response => response.json()).then(value => { document.querySelector("#status").textContent = `Measured ${value.measured}`; });');
      await fs.writeFile(path.join(directory, 'app.css'), '#status { color: rgb(0, 96, 64); }');
      await fs.writeFile(path.join(directory, 'index.html'), `<!doctype html><meta charset="utf-8"><title>Release fixture ${width}</title><link rel="stylesheet" href="/app.css"><main id="status" data-release="${width}"></main><script src="/app.js"></script>`);
      await page.reload();
      await page.waitForFunction((expected) => document.querySelector('#status').textContent === expected, `Measured ${measured}`);
      assert.equal(await page.locator('#status').evaluate((element) => getComputedStyle(element).color), 'rgb(0, 96, 64)');
      assert.equal(await page.locator('#status').getAttribute('data-release'), String(width));
      assert.equal(await page.title(), `Release fixture ${width}`);
      assert.deepEqual(errors, []);
      await context.close();
    }

    // Use the actual browser loader and every published auth catalog, with no
    // intercepted requests or message substitutes. Namespace IDs stay stable
    // when the manifest maps them to a credential-safe asset basename.
    await fs.cp(new URL('../../console/locales/', import.meta.url), path.join(directory, 'locales'), { recursive: true });
    for (const file of ['i18n.mjs', 'i18n-core.mjs']) {
      await fs.copyFile(new URL('../../console/' + file, import.meta.url), path.join(directory, file));
    }
    await fs.writeFile(path.join(directory, 'index.html'), '<!doctype html><meta charset="utf-8"><title>Auth message fixture</title><header><div class="who"></div></header><main id="message"></main><script type="module">import { i18n } from "/i18n.mjs"; await i18n.ready; await i18n.ensure("auth"); i18n.text(document.querySelector("#message"), "auth.googleSignIn"); window.authReady = true;</script>');
    const manifest = JSON.parse(await fs.readFile(path.join(directory, 'locales/manifest.json'), 'utf8'));
    const context = await browser.newContext({ locale: 'en', viewport: { width: 1440, height: 900 } });
    const page = await context.newPage();
    const errors = [], requests = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('request', request => requests.push(new URL(request.url()).pathname));
    await page.goto(`http://127.0.0.1:${server.address().port}/`);
    await page.waitForFunction(() => window.authReady === true);
    let lastMessage;
    for (const entry of manifest.locales.filter(entry => entry.status === 'enabled')) {
      const messages = Object.assign({}, ...await Promise.all(entry.files.auth.map(file => fs.readFile(path.join(directory, 'locales', file), 'utf8').then(JSON.parse))));
      await page.evaluate(tag => window.DevCoordinatorI18n.setLocale(tag), entry.tag);
      assert.equal(await page.locator('html').getAttribute('lang'), entry.tag, entry.tag);
      assert.equal(await page.locator('#message').textContent(), messages.googleSignIn, entry.tag);
      for (const file of entry.files.auth) assert.ok(requests.includes('/locales/' + file), file);
      lastMessage = messages.googleSignIn;
    }
    await page.reload();
    await page.waitForFunction(() => window.authReady === true);
    assert.equal(await page.locator('#message').textContent(), lastMessage);
    assert.ok(!requests.some(file => file.endsWith('/auth.json')));
    assert.deepEqual(errors, []);
    await context.close();
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(directory, { recursive: true, force: true });
  }
});
