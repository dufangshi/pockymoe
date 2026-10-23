import { test, expect, type Browser, type TestInfo } from '@playwright/test';
import { mkdir, mkdtemp } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';
import { Fixture } from './helpers/account-key-fixture';

// No auth material enters traces, videos or automatic failure screenshots.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

const freePort = () => new Promise<number>(done => {
  const server = createServer();
  server.listen(0, '127.0.0.1', () => { const port = (server.address() as { port: number }).port; server.close(() => done(port)); });
});

// The fixture's relay normally takes E2E_API_PORT, which the Playwright
// Supervisor already uses; this relay gets its own port.
async function isolatedFixture(browser: Browser, info: TestInfo) {
  const port = await freePort();
  class Isolated extends Fixture { override readonly base = `http://localhost:${port}`; }
  const dataRoot = resolve(process.env.E2E_WORKSPACE_ROOT ?? '.scratch');
  await mkdir(dataRoot, { recursive: true });
  const root = await mkdtemp(join(dataRoot, 'cross-device-tabs-e2e-'));
  await mkdir(join(root, 'home'));
  const f = new Isolated(root, browser, info);
  f.start('relay', { PORT: String(port), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_PUBLIC_BASE_URL: f.base,
    POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist') });
  await expect.poll(() => fetch(f.base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
  return f;
}

// A tab on another device switches the conversation in place: the workbench
// (sidebar, tabs, composer) never leaves the page, so it never flashes blank.
test('a tab on another device switches without blanking the page', async ({ browser }, info) => {
  test.setTimeout(300_000);
  const f = await isolatedFixture(browser, info);
  try {
    const owner = await f.account('tabs-owner');
    const d1 = await f.device(owner, 1), d2 = await f.device(owner, 2);
    await f.bind(owner, d1);
    await f.bind(owner, d2);
    const page = owner.page;
    const prompt = page.getByRole('textbox', { name: 'Prompt', exact: true });
    await page.goto(d1.url);
    await expect(prompt).toBeVisible();
    await page.goto(d2.url);
    await expect(prompt).toBeVisible();
    const tabs = page.locator('.matter-thread-tabs .matter-group-tab');
    await expect(tabs.filter({ hasText: 'Signed thread 1' })).toBeVisible();

    // Every frame from the click until the other device's conversation shows.
    await page.evaluate(() => {
      const w = window as typeof window & { __blank?: number; __frames?: number; __stop?: boolean };
      w.__blank = 0; w.__frames = 0; w.__stop = false;
      const tick = () => {
        if (w.__stop) return;
        w.__frames! += 1;
        if (!document.querySelector('.matter-workbench .matter-tabs-row')) {
          w.__blank! += 1;
          const seen = (w as unknown as { __seen?: string[] }).__seen ??= [];
          const summary = `${[...document.querySelectorAll('body *')].slice(0, 6).map(e => (e.className?.toString() || e.tagName).split(' ')[0]).join('>')} | ${document.body.innerText.replace(/\s+/g, ' ').slice(0, 80)} | ${location.pathname.slice(0, 60)}`;
          if (seen.at(-1) !== summary) seen.push(summary);
        }
        requestAnimationFrame(tick);
      };
      tick();
    });
    for (const [title, device] of [['Signed thread 1', d1], ['Signed thread 2', d2], ['Signed thread 1', d1]] as const) {
      await tabs.filter({ hasText: title }).click();
      await expect(page).toHaveURL(new RegExp(`/devices/${device.id}/threads/${device.thread}`));
      await expect(page.locator('.matter-group-tab[aria-current="page"]')).toContainText(title);
      await expect(prompt).toBeVisible();
      await page.waitForTimeout(800);
    }
    const sampled = await page.evaluate(() => {
      const w = window as typeof window & { __blank?: number; __frames?: number; __stop?: boolean };
      w.__stop = true;
      return { blank: w.__blank, frames: w.__frames, seen: (w as unknown as { __seen?: string[] }).__seen ?? [] };
    });
    expect(sampled.frames).toBeGreaterThan(10);
    expect(sampled.blank, `frames without the workbench while switching devices: ${JSON.stringify(sampled.seen)}`).toBe(0);
    // The conversation is the other device's, and it can still be written to.
    await prompt.fill('cross-device marker');
    await page.keyboard.press('Control+Enter');
    await expect(page.locator('[data-timeline-turn]').filter({ hasText: 'cross-device marker' }).first()).toBeVisible({ timeout: 30_000 });
  } finally {
    await f.close();
  }
});
