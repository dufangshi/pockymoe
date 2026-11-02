import { test, expect, _electron as electron, type ElectronApplication } from '@playwright/test';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fixture, sendPrompt } from '../helpers/account-key-fixture';
const appRoot = process.env.POCKYMOE_ELECTRON_APP_ROOT;
const evidence = '.scratch/device-management/review-electron';
test.use({ trace: 'off', screenshot: 'off', video: 'off' });
test('real dev Electron retains thread frames and clears A history before B navigates Back', async ({ browser }, info) => {
  test.setTimeout(180000);
  if (!appRoot) throw new Error('Set POCKYMOE_ELECTRON_APP_ROOT to an independent built desktop checkout');
  if (process.env.E2E_API_PORT === '45679') throw new Error('Use an isolated relay port');
  mkdirSync(evidence, { recursive: true });
  const f = await fixture(browser, info);
  let app: ElectronApplication | undefined;
  try {
    const owner = await f.account('electron-owner'), other = await f.account('electron-other');
    const device = await f.device(owner, 1, { PORT: '16382', POCKYMOE_RELAY_SUPERVISOR_PORT: '16382' });
    await f.bind(owner, device);
    await owner.page.goto(device.url); await sendPrompt(owner.page, 'ELECTRON_RETAINED_HISTORY');
    const requireApp = createRequire(resolve(appRoot, 'package.json'));
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX|ELECTRON)_/.test(key))) as Record<string, string>;
    Object.assign(env, { POCKYMOE_DESKTOP_E2E: '1', POCKYMOE_DESKTOP_USER_DATA: join(f.root, 'electron-profile'), POCKYMOE_DESKTOP_WEB_ORIGIN: f.base, POCKYMOE_DESKTOP_RELAY_ORIGIN: f.base, POCKYMOE_DESKTOP_THEME: 'dark' });
    app = await electron.launch({ executablePath: requireApp('electron'), args: [resolve(appRoot, 'out/main/index.js')], env });
    await app.context().addInitScript(({ users }) => {
      localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem('remote-codex-theme-mode', 'dark');
      for (const id of users) localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
    }, { users: [owner.userId, other.userId] });
    await expect.poll(() => app!.windows().some(page => page.url().startsWith(f.base))).toBe(true);
    const page = app.windows().find(page => page.url().startsWith(f.base))!;
    const win = await app.browserWindow(page); await win.evaluate(window => window.setBounds({ width: 1440, height: 1000 }));
    const login = async (account: typeof owner, initial = false) => {
      try {
        if (initial) await page.goto(f.base + '/relay-portal?returnTo=%2F');
        await page.getByLabel('Email or username', { exact: true }).fill(account.username);
        await page.getByLabel('Password', { exact: true }).fill(account.password);
        await page.getByRole('button', { name: 'Sign in', exact: true }).click();
        await expect(page.locator('[data-desktop-surface]')).toBeVisible();
      } catch { await page.goto('about:blank'); throw new Error('Isolated Electron sign-in failed; auth fields were not retained'); }
    };
    await login(owner, true);
    await page.evaluate(() => { (window as any).__electronRealm = 'same-renderer'; });
    expect(await page.evaluate(async () => (await window.pockymoeHost!.capabilities()).surface)).toBe('desktop');
    const go = (path: string) => app!.evaluate((_electron, path) => { void (globalThis as any).__pockymoeDesktopTest.windows.dispatch({ type: 'navigate', path }); }, path);
    const threadPath = new URL(device.url).pathname;
    await go(threadPath); await expect(page.getByText('ELECTRON_RETAINED_HISTORY', { exact: true })).toBeVisible();
    await page.getByTestId('thread-scroll-container').evaluate(el => el.setAttribute('data-electron-retained', 'yes'));
    const cdp = await page.context().newCDPSession(page); let frames = 0;
    cdp.on('Page.screencastFrame', async frame => { writeFileSync(`${evidence}/frame-${String(frames++).padStart(4, '0')}.jpg`, Buffer.from(frame.data, 'base64')); await cdp.send('Page.screencastFrameAck', { sessionId: frame.sessionId }).catch(() => {}); });
    await cdp.send('Page.startScreencast', { format: 'jpeg', quality: 65, everyNthFrame: 1 });
    await page.evaluate(() => {
      (window as any).__electronSamples = []; (window as any).__sampleElectron = true;
      const begin = performance.now();
      const sample = () => {
        const visible = (el: Element) => el.getClientRects().length > 0 && !el.closest('[hidden]');
        const thread = [...document.querySelectorAll('[data-testid="thread-scroll-container"]')].some(visible);
        const devices = [...document.querySelectorAll('[data-testid="desktop-devices"]')].some(visible);
        const map = [...document.querySelectorAll('.desktop-map-scene:not([hidden]) .thread-map')].some(visible);
        (window as any).__electronSamples.push({ ms: performance.now() - begin, path: location.pathname, content: thread || devices || map, thread });
        if ((window as any).__sampleElectron) requestAnimationFrame(sample);
      }; sample();
    });
    for (let i = 0; i < 3; i++) {
      await go('/desktop/devices'); await expect(page.getByTestId('desktop-devices')).toBeVisible();
      await go(threadPath); await expect(page.getByText('ELECTRON_RETAINED_HISTORY', { exact: true })).toBeVisible();
      await expect(page.locator('[data-electron-retained]')).toBeVisible();
    }
    await go('/'); await expect(page.locator('.desktop-map-scene:not([hidden]) .thread-map')).toBeVisible();
    await go(threadPath); await expect(page.locator('[data-electron-retained]')).toBeVisible();
    const samples = await page.evaluate(() => { (window as any).__sampleElectron = false; return (window as any).__electronSamples; });
    await cdp.send('Page.stopScreencast');
    expect(samples.filter((sample: any) => !sample.content)).toEqual([]);
    const version = await app.evaluate(() => ({ electron: process.versions.electron, chrome: process.versions.chrome }));
    writeFileSync(`${evidence}/frames.json`, JSON.stringify({ version, frames, samples }, null, 2));
    await page.screenshot({ path: `${evidence}/retained-thread.png` });
    await go('/desktop/devices'); await expect(page.getByTestId('desktop-devices')).toBeVisible();
    await page.locator('.desktop-map-scene:not([hidden]) .matter-rail').getByRole('button', { name: /Relay account menu/ }).click();
    await page.getByRole('menuitem', { name: 'Log out', exact: true }).click();
    await expect(page.locator('[data-electron-retained]')).toHaveCount(0);
    await login(other);
    expect(await page.evaluate(() => (window as any).__electronRealm)).toBe('same-renderer');
    await page.evaluate(path => {
      (window as any).__leakedA = false;
      new MutationObserver(() => { if (document.body.textContent?.includes('ELECTRON_RETAINED_HISTORY')) (window as any).__leakedA = true; }).observe(document.body, { subtree: true, childList: true, characterData: true });
      history.replaceState({}, '', path); history.pushState({}, '', '/desktop/devices'); dispatchEvent(new PopStateEvent('popstate'));
    }, threadPath);
    await page.goBack(); await expect(page).toHaveURL(device.url);
    await expect(page.locator('.desktop-route:not([hidden])')).toBeVisible();
    await expect(page.getByText('ELECTRON_RETAINED_HISTORY', { exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => (window as any).__leakedA)).toBe(false);
    writeFileSync(`${evidence}/account-boundary.json`, JSON.stringify({ sameElectronProcess: true, sameRenderer: true, backToOldThreadDenied: true, oldHistoryEverRendered: false }));
  } finally { if (app) await app.close(); await f.close(); }
});
