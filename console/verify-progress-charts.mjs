import path from 'node:path';

export async function verifyProgressCharts({ page, daemon, check, scenario, baseUrl, output, settle }) {
  const failures = [];
  const onError = (error) => failures.push(error.message);
  page.on('pageerror', onError);
  const originalViewport = page.viewportSize();
  daemon.setScenario(scenario);
  await page.goto(`${baseUrl}#/progress/r0123456789abcdef`);
  await page.waitForSelector('.progress-bar-line-chart');
  await page.evaluate(() => document.fonts.ready);
  for (const viewport of [{ width: 1110, height: 876 }, { width: 390, height: 844 }]) {
    await page.setViewportSize(viewport);
    for (const period of ['hour', 'day', 'week']) {
      const prefix = `Progress ${viewport.width} ${period}`;
      daemon.calls.length = 0;
      await page.click(`[data-progress-period="${period}"]`);
      await settle(period);
      check(`${prefix}: period control reads the requested real API shape`, daemon.calls.some((call) =>
        call.operation === 'progress.repository' && call.params.period === period));
      for (const scrollToEnd of [false, true]) {
        await page.locator('.progress-chart-scroll').evaluate((element, atEnd) => {
          element.scrollLeft = atEnd ? element.scrollWidth : 0;
        }, scrollToEnd);
        const geometry = await page.locator('.progress-bar-line-chart').evaluateAll((charts) => charts.map((chart) => {
          const headings = [...chart.querySelectorAll('[data-ui-verify-svg-overlap]')];
          const plotted = [...chart.querySelectorAll('.progress-bar,.progress-bar-value')];
          const intersects = (first, second) => Math.min(first.right, second.right) - Math.max(first.left, second.left) > 1
            && Math.min(first.bottom, second.bottom) - Math.max(first.top, second.top) > 1;
          const completed = [...chart.querySelectorAll('.progress-completed-bar')];
          return {
            headings: headings.length,
            renderedScale: chart.getScreenCTM()?.a,
            collisions: headings.flatMap((heading) => plotted.filter((item) =>
              intersects(heading.getBoundingClientRect(), item.getBoundingClientRect()))).length,
            firstMaximum: completed.length > 0 && Number(completed[0].getAttribute('height'))
              === Math.max(...completed.map((bar) => Number(bar.getAttribute('height')))),
            compactValue: chart.querySelector('.progress-bar-value')?.textContent,
          };
        }));
        check(`${prefix}: protected labels clear maximum bars at ${scrollToEnd ? 'end' : 'start'}`,
          geometry.length === 2 && geometry.every((chart) => chart.headings === 2 && chart.collisions === 0 && chart.firstMaximum),
          JSON.stringify(geometry));
        check(`${prefix}: chart text is not compressed below its declared size`, geometry.every((chart) => chart.renderedScale >= .99));
        check(`${prefix}: first planned-line value uses compact thousands`, /5[.,]5\s?k/i.test(geometry[1]?.compactValue || ''));
      }
      await page.locator('.progress-chart-scroll').evaluate((element) => { element.scrollLeft = 0; });
      await page.evaluate(() => window.scrollTo(0, 0));
      check(`${prefix}: no document overflow`, await page.evaluate(() =>
        document.documentElement.scrollWidth <= document.documentElement.clientWidth));
      await page.screenshot({ path: path.join(output, `progress-${viewport.width}-${period}-initial.png`) });
      await page.screenshot({ path: path.join(output, `progress-${viewport.width}-${period}-full.png`), fullPage: true });
    }
  }
  check('Progress: no browser runtime errors', failures.length === 0, JSON.stringify(failures));
  page.off('pageerror', onError);
  await page.setViewportSize(originalViewport);
  daemon.calls.length = 0;
  await page.click('[data-progress-period="day"]');
  await settle('day');
}
