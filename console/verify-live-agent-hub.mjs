// Exercise the installed Console with read-only operations. The isolated
// Agent hub journey exercises key rotation, replies and destructive controls.
import assert from 'node:assert/strict';
import path from 'node:path';
import { createRequire } from 'node:module';

export async function verifyLiveAgentHub({ root, out, base, request, check }) {
  const commit = process.env.CONSOLE_VERIFY_SOURCE_COMMIT;
  assert.match(commit || '', /^[a-f0-9]{40}$/);
  const read = async (operation, params = {}) => {
    const response = await request(`/api/v3/${operation}`, JSON.stringify(params));
    const reply = JSON.parse(response.body);
    assert.equal(response.status, 200, operation);
    assert.equal(reply.ok, true, operation);
    return reply.data;
  };
  const [ping, settings, listing] = await Promise.all([
    read('ping'),
    read('agent.hub.settings.get'),
    read('agent.hub.console.rooms', { space: 'private', limit: 50 }),
  ]);
  check('The installed hub daemon reports the expected source', ping.source_commit === commit);

  const runtime = process.env.CONSOLE_VERIFY_PLAYWRIGHT || path.join(root, 'ci/playwright');
  const { chromium } = createRequire(path.join(runtime, 'package.json'))('playwright');
  const browser = await chromium.launch({ args: [`--host-resolver-rules=MAP ${base.hostname} 127.0.0.1`] });
  try {
    const page = await browser.newPage({ viewport: { width: 1487, height: 1058 }, colorScheme: 'light', locale: 'en-GB' });
    const errors = [];
    page.on('pageerror', error => errors.push(error.name));
    const response = await page.goto(base.href + '#/agent-hub');
    check('The Console page carries the running release identity', response?.headers()['x-ui-source-revision'] === `git:${commit}`);
    const button = page.locator('[data-hub-action="generate-key"]');
    await button.waitFor();
    await page.waitForFunction(() => document.querySelector('[data-hub-action="generate-key"]')?.disabled === false);
    check('All four Guided Handoff steps are reachable', await page.locator('.hub-steps [data-hub-step]').count() === 4);
    check('The administrator sees the action for the stored key state',
      (await button.textContent()).trim() === (settings.current_key_created_at ? 'Rotate instance key' : 'Generate instance key'));
    check('Opening setup reveals no key material', await page.locator('[data-hub-secret]').count() === 0);
    const mask = [page.locator('#who-email'), page.locator('[data-hub-secret]')];
    await page.screenshot({ path: path.join(out, 'setup-wide.png'), mask });

    await page.locator('.hub-steps [data-hub-step="connect"]').click();
    await page.locator('#hub-client').selectOption('remote');
    check('Private remote onboarding uses this installation’s HTTPS endpoint',
      (await page.locator('.hub-panel').innerText()).includes(base.origin + '/.well-known/devcoordinator2/agent-hub'));

    await page.locator('.hub-steps [data-hub-step="rooms"]').click();
    const room = listing.rooms[0];
    if (room) {
      await page.locator(`[data-hub-room="${room.room_id}"]`).click();
      await page.locator('#hub-reply').waitFor();
      const { deliveries } = await read('agent.hub.console.receive', { space: 'private', room_id: room.room_id, limit: 50 });
      for (const { message, receipt_counts: counts } of deliveries) {
        assert.ok(counts && counts.acknowledged <= counts.recipients, 'Valid recipient counts');
        const item = page.locator(`[data-hub-message="${message.message_id}"]`);
        await item.waitFor();
        const expected = await page.evaluate(counts => window.DevCoordinatorI18n.t(
          counts.recipients ? 'agent-hub.receipts' : 'agent-hub.noRecipients', counts), counts);
        check('A rendered message shows its recorded recipient acknowledgements',
          (await item.locator('[data-hub-receipt]').innerText()).includes(expected));
      }
      check('The human reply composer is available in a real private room', await page.locator('#hub-reply').isVisible());
    } else {
      await page.locator('.hub-room-list .hub-empty').waitFor();
      check('The installed empty room list is explicit', await page.locator('[data-hub-room]').count() === 0);
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page.locator('.hub-steps [data-hub-step="key"]').click();
    await button.waitFor();
    check('Phone setup fits the viewport', await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1));
    await page.screenshot({ path: path.join(out, 'setup-phone.png'), mask });
    check('The installed hub journey runs without browser exceptions', errors.length === 0);
  } finally {
    await browser.close();
  }
}
