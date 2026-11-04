import { expect, type BrowserContext, type Page, type ElectronApplication } from '@playwright/test';
import { readFile, writeFile, rename } from 'node:fs/promises';
import { basename, join } from 'node:path';

type Fact = { event: string; [key: string]: unknown };
export async function exerciseImageAgeing(input: {
  page: Page; context: BrowserContext; desktop?: ElectronApplication;
  workerFile: string; relay: string; clockFile: string; png: Buffer;
  source: string; facts: Fact[]; baseline: boolean; workspace: string;
}) {
  const { page, context, desktop, facts } = input;
  const cdp = await context.newCDPSession(page);
  const probeRequests = new Map<string, string>();
  const probeStatuses = new Map<string, number>();
  cdp.on('Network.requestWillBeSent', ({ requestId, request }) => {
    const label = new URL(request.url).searchParams.get('_ageProbe');
    if (label) probeRequests.set(requestId, label);
  });
  cdp.on('Network.responseReceivedExtraInfo', ({ requestId, statusCode }) => {
    const label = probeRequests.get(requestId);
    if (label) probeStatuses.set(label, statusCode);
  });
  await cdp.send('Network.enable');
  const versions = new Map<string, string>();
  let stops = 0;
  cdp.on('ServiceWorker.workerVersionUpdated', ({ versions: updates }) => {
    for (const v of updates) { versions.set(v.versionId, v.runningStatus); if (v.runningStatus === 'stopped') stops++; }
  });
  await cdp.send('ServiceWorker.enable');
  const probe = async (label: string) => {
    const url = new URL(input.source);
    url.searchParams.set('_ageProbe', label);
    const responses: import('@playwright/test').Response[] = [];
    const capture = (response: import('@playwright/test').Response) => { if (new URL(response.url()).searchParams.get('_ageProbe') === label) responses.push(response); };
    page.on('response', capture);
    const decoded = await page.evaluate(url => new Promise<boolean>(resolve => {
      const image = new Image();
      const timer = setTimeout(() => { image.remove(); resolve(false); }, 18_000);
      image.onload = () => { clearTimeout(timer); image.remove(); resolve(true); };
      image.onerror = () => { clearTimeout(timer); image.remove(); resolve(false); };
      document.body.append(image); image.src = url;
    }), url.href);
    page.off('response', capture);
    const reply = responses.at(-1);
    const result = { event: 'age-native', label, decoded, status: reply?.status() ?? probeStatuses.get(label) ?? 0,
      workerControlled: await page.evaluate(() => !!navigator.serviceWorker.controller),
      workerHandled: reply?.fromServiceWorker() ?? false };
    facts.push(result);
    console.log('Age native:', JSON.stringify(result));
    if (!input.baseline && label !== 'worker-bypass') expect(result, label).toMatchObject({ decoded: true, status: 200, workerHandled: true });
    return result;
  };
  const send = async (label: string) => {
    const before = facts.length;
    await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill(`IMAGE_PIPELINE_MARKER AGE_${label}`);
    await page.locator('input[type="file"][accept="image/*"]').setInputFiles({ name: `age-${label}.png`, mimeType: 'image/png', buffer: input.png });
    await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
    await expect(page.locator('[data-role="assistant"]').last()).toContainText(`AGE_${label}`, { timeout: 40_000 });
    const tile = page.locator('[data-role="user"] .recoverable-image').last();
    await expect.poll(() => tile.getAttribute('data-image-state'), { timeout: 25_000 }).toMatch(/ready|failed/);
    const state = await tile.getAttribute('data-image-state');
    const scheme = () => tile.locator('img').evaluateAll(images => {
      const src = images[0]?.getAttribute('src');
      return src ? new URL(src, location.href).protocol : 'none';
    });
    const first = await scheme();
    let retry = 'not-needed';
    if (state === 'failed') {
      await tile.locator('.error-mark').click();
      await page.getByRole('button', { name: 'Retry', exact: true }).click();
      await expect.poll(() => tile.getAttribute('data-image-state'), { timeout: 25_000 }).toMatch(/ready|failed/);
      retry = (await tile.getAttribute('data-image-state')) === 'ready' ? 'recovered' : 'failed';
    }
    const result = { event: 'age-tile', label, state, scheme: first, retry, finalScheme: await scheme(),
      responses: facts.slice(before).filter(f => f.event === 'response') };
    facts.push(result);
    console.log('Age tile:', JSON.stringify(result));
    if (!input.baseline) expect(await tile.getAttribute('data-image-state'), label).toBe('ready');
    return result;
  };
  const workerSource = await readFile(input.workerFile, 'utf8');
  const update = async (source: string) => {
    await writeFile(input.workerFile, source);
    await page.evaluate(async () => {
      const changed = new Promise<void>((resolve, reject) => {
        const timer = setTimeout(() => reject(Error('worker activation timed out')), 15_000);
        navigator.serviceWorker.addEventListener('controllerchange', () => { clearTimeout(timer); resolve(); }, { once: true });
      });
      await (await navigator.serviceWorker.ready).update();
      await changed;
    });
  };
  let second: Page | undefined;
  let browserWindow: number | undefined;
  try {
    await cdp.send('ServiceWorker.stopAllWorkers');
    facts.push({ event: 'worker-statuses-after-stop', versions: [...versions.values()] });
    await expect.poll(() => stops, { timeout: 10_000 }).toBeGreaterThan(0);
    facts.push({ event: 'worker-stopped', observed: true });
    await probe('worker-stop'); await send('worker-stop');
    await update(workerSource + '\n// age-event update\n');
    await probe('worker-update'); await send('worker-update');
    if (desktop) {
      await desktop.evaluate(({ BrowserWindow }, url) => {
        const second = new BrowserWindow({ show: true, webPreferences: { partition: 'persist:remote-codex', sandbox: true, contextIsolation: true, nodeIntegration: false } });
        void second.loadURL(url);
      }, new URL('/relay-account', page.url()).href);
      await expect.poll(() => desktop.windows().some(window => window !== page && window.url().startsWith(new URL(page.url()).origin))).toBe(true);
      second = desktop.windows().find(window => window !== page && window.url().startsWith(new URL(page.url()).origin));
      await desktop.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().find(window => window.webContents.getURL().includes('/threads/'))?.hide());
      await expect.poll(() => desktop.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().find(window => window.webContents.getURL().includes('/threads/'))?.isVisible())).toBe(false);
    } else if (process.env.E2E_VERIFY_HIDDEN === '1') {
      const { targetInfo } = await cdp.send('Target.getTargetInfo');
      const oldPages = new Set(context.pages());
      await cdp.send('Target.createTarget', { url: new URL('/relay-account', page.url()).href, newWindow: true, browserContextId: targetInfo.browserContextId });
      await expect.poll(() => context.pages().some(candidate => !oldPages.has(candidate))).toBe(true);
      second = context.pages().find(candidate => !oldPages.has(candidate));
      browserWindow = (await cdp.send('Browser.getWindowForTarget', { targetId: targetInfo.targetId })).windowId;
      await cdp.send('Browser.setWindowBounds', { windowId: browserWindow, bounds: { windowState: 'minimized' } });
    } else { second = await context.newPage(); await second.goto(new URL('/relay-account', page.url()).href); await second.bringToFront(); }
    expect(second).toBeTruthy();
    await second!.waitForURL(url => url.origin === new URL(page.url()).origin, { timeout: 15_000 });
    await expect.poll(() => second!.evaluate(() => !!navigator.serviceWorker.controller)).toBe(true);
    if (browserWindow !== undefined) await expect.poll(async () => (await cdp.send('Browser.getWindowBounds', { windowId: browserWindow! })).bounds.windowState).toBe('minimized');
    facts.push({ event: 'window-visibility', visibility: await page.evaluate(() => document.visibilityState), nativeVisible: desktop ? await desktop.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().find(window => window.webContents.getURL().includes('/threads/'))?.isVisible()) : null, minimized: desktop ? await desktop.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows().find(window => window.webContents.getURL().includes('/threads/'))?.isMinimized()) : browserWindow !== undefined ? (await cdp.send('Browser.getWindowBounds', { windowId: browserWindow })).bounds.windowState === 'minimized' : null });
    await probe('second-window-hidden'); await send('second-window-hidden');
    facts.push({ event: 'visibility-after-image', visibility: await page.evaluate(() => document.visibilityState) });
    if (browserWindow !== undefined) await cdp.send('Browser.setWindowBounds', { windowId: browserWindow, bounds: { windowState: 'normal' } });
    if (desktop) await desktop.evaluate(({ BrowserWindow }) => { const main = BrowserWindow.getAllWindows().find(window => window.webContents.getURL().includes('/threads/')); main?.restore(); main?.show(); });
    await page.bringToFront();
    if (desktop) await desktop.evaluate(({ powerMonitor }) => { powerMonitor.emit('suspend'); });
    await cdp.send('Page.setWebLifecycleState', { state: 'frozen' });
    await cdp.send('ServiceWorker.stopAllWorkers');
    await cdp.send('Page.setWebLifecycleState', { state: 'active' });
    if (desktop) await desktop.evaluate(({ powerMonitor }) => { powerMonitor.emit('resume'); });
    await probe('simulated-sleep-wake'); await send('simulated-sleep-wake');
    await context.setOffline(true);
    expect(await page.evaluate(async () => { try { await fetch(location.origin + '/offline-check', { cache: 'no-store', signal: AbortSignal.timeout(3_000) }); return false; } catch { return true; } })).toBe(true);
    await context.setOffline(false);
    await probe('network-restored'); await send('network-restored');
    if (input.clockFile) {
      // Only the isolated Supervisor's real-time clock moves. Its signed
      // descriptor proves the old key lifetime (including grace) has elapsed.
      const descriptor = async () => page.evaluate(async url => {
        const target = new URL(url); target.searchParams.set('challenge', crypto.randomUUID());
        const response = await fetch(target, { credentials: 'include', cache: 'no-store', signal: AbortSignal.timeout(10_000) });
        if (!response.ok) throw Error('descriptor status ' + response.status);
        const { expiresAt, serverTime, keyId } = await response.json();
        return { expiresAt, serverTime, keyId } as { expiresAt: number; serverTime: number; keyId: string };
      }, new URL('/relay/devices/' + new URL(input.source).pathname.split('/devices/')[1]!.split('/')[0] + '/api/transport/key?challenge=age', input.relay).href);
      const old = await descriptor();
      await writeFile(input.clockFile, '3665');
      // Advance the open renderer and existing workers as well, as elapsed
      // time would. Server-only skew can reject a stale-key POST, which is a
      // different mechanism from naturally expiring cached descriptors.
      const advance = () => { const original = Date.now.bind(Date); Date.now = () => original() + 3_665_000; };
      await page.evaluate(advance);
      for (const worker of context.serviceWorkers()) await worker.evaluate(advance);
      const fresh = await descriptor();
      expect(fresh.serverTime).toBeGreaterThan(old.expiresAt + 60_000);
      expect(fresh.keyId).not.toBe(old.keyId);
      facts.push({ event: 'key-expired', advancedSeconds: 3665, expiredBeyondGrace: true, rotated: true, rendererAndWorkerClocksAdvanced: true });
      await probe('key-expired'); await send('key-expired');
    } else facts.push({ event: 'key-expiry-not-run', reason: 'E2E_CLOCK_LIBRARY not configured' });
    if (process.env.E2E_IMAGE_AGEING_CORE_ONLY === '1') return;
    // An old mounted page can see a newly deployed worker with a different
    // build default. This exercises the same lost per-client configuration.
    expect(workerSource.includes(input.relay)).toBe(true);
    await update(workerSource.split(input.relay).join('http://localhost:18599') + '\n// changed deployment default\n');
    await probe('worker-new-default'); await send('worker-new-default');
    const stoppedBefore = stops;
    await cdp.send('ServiceWorker.stopAllWorkers');
    await expect.poll(() => stops).toBeGreaterThan(stoppedBefore);
    await probe('cold-worker-other-default'); await send('cold-worker-other-default');
    const retrySource = input.baseline ? null : await page.locator('[data-role="user"] .recoverable-image').last().locator('img').getAttribute('src');
    const retryPath = retrySource && !retrySource.startsWith('blob:') ? new URL(retrySource).searchParams.get('path') : null;
    await update(workerSource + '\n// restore before bypass proof\n');
    // A real active worker without a fetch handler guarantees native reads
    // go raw; Electron ignored CDP's bypass flag in the baseline experiment.
    await update("self.addEventListener('install', e => e.waitUntil(self.skipWaiting())); self.addEventListener('activate', e => e.waitUntil(self.clients.claim())); self.addEventListener('message', e => { if (e.data?.type === 'pockymoe-page-relay') e.ports[0]?.postMessage({ok:true}); });\n");
    const unhandled = await probe('worker-bypass');
    expect(unhandled.workerHandled).toBe(false);
    expect(unhandled.decoded).toBe(false);
    expect(unhandled.status).toBe(403);
    await send('worker-bypass');
    if (!input.baseline) {
      expect(facts.find(f => f.event === 'age-tile' && f.label === 'worker-bypass')?.finalScheme).toBe('blob:');
      // Exercise the real Retry action with a missing source restored while
      // native requests still bypass the worker entirely.
      expect(retryPath).toBeTruthy();
      const attachment = join(input.workspace, retryPath!);
      await rename(attachment, attachment + '.held');
      try {
        await cdp.send('Network.clearBrowserCache');
        await page.reload();
        const tile = page.locator('.attachment-tile[title=' + JSON.stringify(basename(retryPath!)) + ']').locator('..');
        await expect(tile.locator('.error-mark')).toBeVisible();
        await rename(attachment + '.held', attachment);
        await tile.locator('.error-mark').click();
        await page.getByRole('button', { name: 'Retry', exact: true }).click();
        await expect(tile).toHaveAttribute('data-image-state', 'ready');
        await expect(tile.locator('img')).toHaveAttribute('src', /^blob:/);
        facts.push({ event: 'bypass-manual-retry', decoded: true, scheme: 'blob:', workerHandled: false });
      } finally { await rename(attachment + '.held', attachment).catch(() => {}); }
    }

  } catch (error) {
    facts.push({ event: 'age-error', message: String(error) });
    throw error;
  } finally {
    await context.setOffline(false);
    // The isolated profile closes after this test; restore the served bytes
    // without requiring one more worker activation during teardown.
    await writeFile(input.workerFile, workerSource);
    await second?.close();
    await cdp.detach();
  }
}
