import { _electron as electron, expect, test, type BrowserContext, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { deflateSync } from 'node:zlib';
import { createServer, type Server } from 'node:http';
import { mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createServer as createNetServer } from 'node:net';
import { extname, join, normalize, resolve } from 'node:path';

// Production topology with temporary storage and synthetic credentials: an
// API-only relay, the static Web build served like GitHub Pages (no response
// headers, 404.html fallback) on another origin, and a fake-runtime device.
// Set E2E_STATIC_WEB_DIST to reuse a static build made for E2E_RELAY_PORT.
test.describe.configure({ mode: 'serial' });
test.use({ actionTimeout: 15_000 });

const relayPort = Number(process.env.E2E_RELAY_PORT);
const webPort = Number(process.env.E2E_WEB_PORT);
const relay = `http://localhost:${relayPort}`;
const web = `http://localhost:${webPort}`;
const password = randomBytes(24).toString('hex');
// A genuine large RGB PNG generated locally; no private input enters fixtures.
function fixturePng() {
  const chunk = (type: string, body: Buffer) => {
    const data = Buffer.concat([Buffer.from(type), body]);
    let crc = 0xffffffff;
    for (const byte of data) { crc ^= byte; for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1)); }
    const size = Buffer.alloc(4), tail = Buffer.alloc(4);
    size.writeUInt32BE(body.length); tail.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
    return Buffer.concat([size, data, tail]);
  };
  const retina = process.env.E2E_RETINA_PNG === '1';
  const width = retina ? 5120 : 1024, height = retina ? 2880 : 1536, rows = Buffer.alloc((width * 3 + 1) * height);
  let seed = 42;
  for (let y = 0; y < height; y++) for (let x = 1; x <= width * 3; x++) {
    seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5;
    rows[y * (width * 3 + 1) + x] = retina ? ((x >>> 4) ^ (y >>> 4) * 7) & 255 : seed & 255;
  }
  const header = Buffer.alloc(13); header.writeUInt32BE(width); header.writeUInt32BE(height, 4); header[8] = 8; header[9] = 2;
  return Buffer.concat([Buffer.from('89504e470d0a1a0a', 'hex'), chunk('IHDR', header), chunk('IDAT', deflateSync(rows)), chunk('IEND', Buffer.alloc(0))]);
}
type PipelineFact = { event: string; [key: string]: unknown };
const processes: ChildProcess[] = [];
let root = '';
let pages: Server | undefined;
let staticDist = '';
let desktop: Awaited<ReturnType<typeof electron.launch>> | undefined;
let context: BrowserContext;
let page: Page;
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !/^(POCKYMOE|REMOTE_CODEX|VITE)_/.test(name)));

function start(command: string, args: string[], extra: Record<string, string>) {
  const child = spawn(command, args, { cwd: resolve('.'), env: { ...env, HOME: join(root, 'home'), ...extra }, stdio: 'ignore', detached: process.platform !== 'win32' });
  processes.push(child);
  return child;
}
async function waitFor(url: string, child?: ChildProcess) {
  await expect.poll(async () => {
    if (child && child.exitCode !== null) throw new Error(`Fixture process exited with ${child.exitCode}`);
    return fetch(url, { signal: AbortSignal.timeout(1500) }).then((r) => r.status).catch(() => 0);
  }, { timeout: 60_000 }).toBe(200);
}
async function clearMediaCache() {
  const cdp = await context.newCDPSession(page);
  await cdp.send('Network.setCacheDisabled', { cacheDisabled: true });
  await cdp.send('Network.clearBrowserCache');
  await cdp.detach();
}
async function relayCookies(): Promise<Array<{ name: string; value: string; httpOnly: boolean; secure: boolean; sameSite: string }>> {
  return desktop ? desktop.evaluate(({ session }, url) => session.fromPartition('persist:remote-codex').cookies.get({ url }), relay) : context.cookies(relay);
}
async function relayApi(path: string, method = 'GET', body?: unknown) {
  const session = (await relayCookies()).find((cookie) => cookie.name === 'remote_codex_relay_session');
  const response = await fetch(relay + path, { method, headers: {
    origin: web, cookie: `remote_codex_relay_session=${session?.value ?? ''}`,
    ...(body === undefined ? {} : { 'content-type': 'application/json' }),
  }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(10_000) });
  const text = await response.text();
  return { status: response.status, data: text ? JSON.parse(text) : null };
}
const freePort = () => new Promise<number>((done) => {
  const server = createNetServer();
  server.listen(0, '127.0.0.1', () => {
    const { port } = server.address() as { port: number };
    server.close(() => done(port));
  });
});
function servePages(dir: string) {
  const types: Record<string, string> = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2', '.wasm': 'application/wasm', '.webmanifest': 'application/manifest+json' };
  const server = createServer(async (request, response) => {
    let file = join(dir, normalize(decodeURIComponent(new URL(request.url ?? '/', web).pathname)).replace(/^(\.\.[/\\])+/, ''));
    let status = 200;
    try {
      if ((await stat(file)).isDirectory()) file = join(file, 'index.html');
      await stat(file);
    } catch { file = join(dir, '404.html'); status = 404; }
    response.writeHead(status, { 'content-type': types[extname(file)] ?? 'application/octet-stream', 'cache-control': 'no-cache' });
    response.end(await readFile(file));
  });
  return new Promise<Server>((done) => server.listen(webPort, 'localhost', () => done(server)));
}

test.beforeAll(async ({ browser }) => {
  test.setTimeout(240_000);
  expect(Number.isInteger(relayPort) && relayPort > 0, 'Set assigned E2E_RELAY_PORT').toBe(true);
  expect(Number.isInteger(webPort) && webPort > 0 && webPort !== relayPort, 'Set assigned E2E_WEB_PORT').toBe(true);
  root = await mkdtemp(join(tmpdir(), 'pockymoe-static-origin-'));
  await mkdir(join(root, 'home'));
  let dist = process.env.E2E_STATIC_WEB_DIST ?? '';
  if (!dist) {
    dist = join(root, 'web');
    const build = start('corepack', ['pnpm', '--filter', '@pockymoe/supervisor-web', 'exec', 'vite', 'build', '--mode', 'static', '--outDir', dist], {
      VITE_POCKYMOE_RELAY_ORIGIN: relay, POCKYMOE_WEB_DOMAIN: 'app.example.test',
    });
    const code = await new Promise<number | null>((done) => build.once('exit', done));
    expect(code, 'Static Web build').toBe(0);
  }
  const relayProcess = start(resolve('target/debug/pockymoe'), ['relay'], {
    POCKYMOE_ADMIN_USERNAME: 'smoke-admin',
    POCKYMOE_ADMIN_PASSWORD: randomBytes(24).toString('hex'),
    POCKYMOE_RELAY_HOST: '127.0.0.1',
    POCKYMOE_RELAY_PORT: String(relayPort),
    POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
    POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
    POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
    POCKYMOE_PUBLIC_BASE_URL: relay,
    POCKYMOE_WEB_ORIGIN: web,
  });
  await waitFor(`${relay}/healthz`, relayProcess);
  staticDist = resolve(dist);
  pages = await servePages(staticDist);
  const { viewport, deviceScaleFactor, isMobile, hasTouch, userAgent } = test.info().project.use;
  if (process.env.E2E_ELECTRON_EXECUTABLE) {
    desktop = await electron.launch({ executablePath: process.env.E2E_ELECTRON_EXECUTABLE, args: [], env: {
      ...env, POCKYMOE_DESKTOP_E2E: '1', POCKYMOE_DESKTOP_USER_DATA: join(root, 'electron-profile'),
      POCKYMOE_DESKTOP_WEB_ORIGIN: web, POCKYMOE_DESKTOP_RELAY_ORIGIN: relay,
    } });
    context = desktop.context();
  } else context = await browser.newContext({ locale: 'en-US', viewport, deviceScaleFactor, isMobile, hasTouch, userAgent });
  await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  if (desktop) {
    await expect.poll(() => desktop!.windows().some(p => p.url().startsWith(web))).toBe(true);
    page = desktop.windows().find(p => p.url().startsWith(web))!;
  } else page = await context.newPage();
});

test.afterAll(async () => {
  if (desktop) await desktop.close(); else await context?.close();
  await new Promise<void>((done) => (pages ? pages.close(() => done()) : done()));
  for (const child of processes.reverse()) {
    if (child.exitCode !== null) continue;
    const exited = new Promise<void>((done) => child.once('exit', () => done()));
    if (child.pid && process.platform !== 'win32') {
      try { process.kill(-child.pid, 'SIGTERM'); } catch { /* process group already exited */ }
    } else child.kill('SIGTERM');
    await exited;
  }
  if (root && process.env.E2E_KEEP_FIXTURE !== '1') await rm(root, { recursive: true, force: true });
});

test('separate static origin registers with a relay cookie and loads devices', async () => {
  const redirect = await fetch(`${relay}/devices`, { redirect: 'manual' });
  expect(redirect.status).toBe(307);
  expect(redirect.headers.get('location')).toBe(`${web}/devices`);
  expect(redirect.headers.has('service-worker-allowed')).toBe(false);
  const apiRequests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.startsWith('/relay/')) apiRequests.push(url.origin);
  });
  await page.goto(`${web}/relay-portal?returnTo=%2Frelay-account`);
  await page.getByRole('button', { name: 'Create account', exact: true }).click();
  await page.getByLabel('Email', { exact: true }).fill('static@example.test');
  await page.getByLabel('Username', { exact: true }).fill('static-smoke-user');
  await page.getByLabel('Password', { exact: true }).fill(password);
  await page.getByLabel('Confirm password', { exact: true }).fill(password);
  const registration = page.waitForResponse((response) => response.url() === `${relay}/relay/auth/register` && response.request().method() === 'POST');
  await page.locator('form button[type="submit"]').click();
  const registered = await registration;
  expect(registered.status()).toBe(200);
  expect(await registered.headerValue('access-control-allow-origin')).toBe(web);
  expect(await registered.headerValue('access-control-allow-credentials')).toBe('true');
  const group = page.getByRole('group', { name: 'Save your recovery code', exact: true });
  const recovery = await group.evaluate((element) => [...element.querySelectorAll('span')].map((span) => span.textContent?.trim()).join('-'));
  await page.getByLabel('Type the last 4 characters to confirm', { exact: true }).fill(recovery.slice(-4));
  await page.getByRole('button', { name: /^I.ve saved it$/ }).click();
  await expect(page.getByRole('heading', { name: 'Sign-in and security', exact: true })).toBeVisible();
  const userId = (await relayApi('/relay/auth/session')).data.user.id;
  await context.addInitScript((id) => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`,
    JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), userId);
  expect((await relayApi('/relay/devices', 'POST', { name: 'Cross-origin smoke device' })).status).toBe(200);
  await page.goto(`${web}/relay-devices`);
  await expect(page.getByRole('article', { name: 'Cross-origin smoke device', exact: true })).toBeVisible();
  const session = (await relayCookies()).find((cookie) => cookie.name === 'remote_codex_relay_session');
  expect(Boolean(session?.httpOnly && session.secure && session.sameSite.toLowerCase() === 'lax')).toBe(true);
  expect(apiRequests.length).toBeGreaterThan(2);
  expect(apiRequests.every((origin) => origin === relay)).toBe(true);
  expect(new URL(page.url()).origin).toBe(web);
});


test('real composer image pipeline survives settlement, switch and Retry', async () => {
  test.setTimeout(180_000);
  const created = await relayApi('/relay/devices', 'POST', { name: 'Image pipeline device' });
  expect(created.status).toBe(200);
  const deviceId: string = created.data.device.id;
  const port = String(await freePort());
  const supervisor = start(resolve(process.env.E2E_SUPERVISOR_BINARY ?? 'target/debug/pockymoe'), ['relay-supervisor'], {
    HOST: '127.0.0.1', PORT: port, POCKYMOE_RELAY_SUPERVISOR_PORT: port,
    POCKYMOE_RELAY_SERVER_URL: relay, POCKYMOE_RELAY_AGENT_TOKEN: created.data.token,
    POCKYMOE_DATABASE_PATH: join(root, 'device.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces'),
    POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_E2E_FAKE_RUNTIME: '1',
    POCKYMOE_ADMIN_USERNAME: 'device-admin', POCKYMOE_ADMIN_PASSWORD: randomBytes(24).toString('hex'),
  });
  const api = `/relay/devices/${deviceId}/api`;
  await expect.poll(async () => {
    if (supervisor.exitCode !== null) throw new Error(`Supervisor exited with ${supervisor.exitCode}`);
    return (await relayApi(`${api}/version`)).status;
  }, { timeout: 45_000 }).toBe(200);
  const work = join(root, 'workspace');
  await mkdir(work);
  const workspace = (await relayApi(`${api}/workspaces`, 'POST', { label: 'Image workspace', absPath: work })).data;
  const started = (await relayApi(`${api}/threads/start`, 'POST', { workspaceId: workspace.id, model: 'fake', title: 'Image thread' })).data;
  const threadId = started.thread?.id ?? started.id;
  const other = (await relayApi(`${api}/threads/start`, 'POST', { workspaceId: workspace.id, model: 'fake', title: 'Other image thread' })).data;
  const otherId = other.thread?.id ?? other.id;
  // Protect the device, so every image must traverse the real encrypted worker path.
  await page.goto(`${web}/relay-devices`);
  await page.getByRole('button', { name: 'Protect all devices', exact: true }).click();
  await page.getByRole('dialog', { name: 'Protect all devices', exact: true }).getByRole('button', { name: 'Protect devices', exact: true }).click();
  const unlock = page.getByRole('dialog', { name: /^(Unlock this browser|Confirm with your password)$/ });
  if (await unlock.waitFor({ timeout: 5_000 }).then(() => true, () => false)) {
    await unlock.getByLabel('Password', { exact: true }).fill(password);
    await unlock.getByRole('button', { name: 'Unlock', exact: true }).click();
  }
  await expect.poll(async () => Boolean((await relayApi(`${api}/transport/key`)).data.certificate), { message: 'device certificate installation completes' }).toBe(true);
  await expect.poll(async () => (await relayApi(`${api}/version`)).status, { message: 'device rejects plaintext after protection' }).toBe(403);
  const facts: PipelineFact[] = [];
  page.on('console', message => { if (message.type() === 'info' && message.text().startsWith('chat-image-failure')) facts.push({ event: 'diagnosis', text: message.text() }); });
  page.on('response', response => {
    if (new URL(response.url()).pathname.endsWith('/assets/image')) facts.push({ event: 'response', status: response.status(), type: response.headers()['content-type'], worker: response.fromServiceWorker(), retry: new URL(response.url()).searchParams.has('_imageRetry') });
  });
  page.on('requestfailed', request => {
    if (new URL(request.url()).pathname.endsWith('/assets/image')) facts.push({ event: 'requestfailed', error: request.failure()?.errorText });
  });
  await page.goto(`${web}/devices/${deviceId}/threads/${threadId}`);
  const composer = page.getByTestId('chat-composer');
  await expect(composer).toBeVisible();
  const later = page.getByRole('button', { name: 'Later', exact: true });
  if (await later.isVisible()) await later.click();
  await page.evaluate(() => {
    const facts: PipelineFact[] = [];
    (window as unknown as { __imageFacts?: PipelineFact[] }).__imageFacts = facts;
    const observe = () => {
      for (const img of document.querySelectorAll<HTMLImageElement>('[data-role="user"] .attachment-tile img')) {
        const src = img.getAttribute('src') ?? '';
        const value = { event: 'tile', src: src.startsWith('blob:') ? 'blob' : 'durable', url: src, retry: src.includes('_imageRetry'), state: img.closest('[data-image-state]')?.getAttribute('data-image-state'), width: img.naturalWidth };
        if (JSON.stringify(facts.at(-1)) !== JSON.stringify(value)) facts.push(value);
      }
    };
    new MutationObserver(observe).observe(document.body, { subtree: true, attributes: true, childList: true });
    addEventListener('load', observe, true); addEventListener('error', observe, true);
  });
  const generatedInput = join(root, 'pipeline.png');
  await writeFile(generatedInput, fixturePng());
  if (process.env.E2E_IMAGE_VISUALS === '1' && (process.env.E2E_IMAGE_INPUT || process.env.E2E_IMAGE_INPUT_DIR)) throw new Error('Visual captures use generated inputs only');
  const inputs = process.env.E2E_IMAGE_INPUT_DIR ? (await readdir(process.env.E2E_IMAGE_INPUT_DIR)).filter(name => name.endsWith('.png')).sort().map(name => join(process.env.E2E_IMAGE_INPUT_DIR!, name)) : [process.env.E2E_IMAGE_INPUT ?? generatedInput];
  const input = inputs.at(-1)!;
  await page.getByRole('textbox', { name: 'Prompt', exact: true }).fill('Inspect this PNG IMAGE_PIPELINE_MARKER');
  await page.locator('input[type="file"][accept="image/*"]').setInputFiles(inputs);
  const acknowledgement = page.waitForResponse(r => new URL(r.url()).pathname.endsWith(`/threads/${threadId}/prompt`) && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  const response = await acknowledgement;
  facts.push({ event: 'ack', status: response.status() });
  const tile = page.locator('[data-role="user"] .attachment-tile').last();
  await expect(tile).toBeVisible();
  const image = tile.locator('img');
  try {
    // Multiple photo paths select the fake provider's documented 25-second turn.
    await expect(page.locator('[data-role="assistant"]').last()).toContainText('IMAGE_PIPELINE_MARKER', { timeout: 35_000 });
    await expect(image).toHaveAttribute('src', /\/assets\/image\?/);
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
    expect(facts.some(f => f.event === 'response' && f.status === 200 && f.worker === true)).toBe(true);
    facts.push({ event: 'settled', controller: await page.evaluate(() => !!navigator.serviceWorker.controller) });
    facts.push(...await page.evaluate(() => (window as unknown as { __imageFacts?: PipelineFact[] }).__imageFacts ?? []));
    // The API-only origin has no worker. Read the same native image there
    // with this browser's actual relay cookie (Electron: the same partition).
    let uncontrolled: Page;
    if (desktop) {
      await desktop.evaluate(({ BrowserWindow }, url) => {
        const probe = new BrowserWindow({ show: false, webPreferences: { partition: 'persist:remote-codex', sandbox: true, contextIsolation: true, nodeIntegration: false } });
        void probe.loadURL(url);
      }, `${relay}/healthz`);
      await expect.poll(() => desktop!.windows().some(p => p.url() === `${relay}/healthz`)).toBe(true);
      uncontrolled = desktop.windows().find(p => p.url() === `${relay}/healthz`)!;
    } else {
      uncontrolled = await context.newPage();
      await uncontrolled.goto(`${relay}/healthz`);
    }
    try {
      expect(await uncontrolled.evaluate(() => !!navigator.serviceWorker.controller)).toBe(false);
      const probeUrl = new URL((await image.getAttribute('src'))!);
      probeUrl.searchParams.set('_workerProbe', '1');
      const response = uncontrolled.waitForResponse(r => new URL(r.url()).searchParams.get('_workerProbe') === '1');
      const decoded = await uncontrolled.evaluate(url => new Promise<boolean>(done => {
        const probe = new Image();
        probe.onload = () => { probe.remove(); done(true); };
        probe.onerror = () => { probe.remove(); done(false); };
        document.body.append(probe); probe.src = url;
      }), probeUrl.href);
      const unhandled = await response;
      expect(decoded).toBe(false);
      expect(unhandled.status()).toBe(403);
      expect(unhandled.fromServiceWorker()).toBe(false);
      facts.push({ event: 'without-worker', status: unhandled.status(), type: unhandled.headers()['content-type'] });
    } finally { await uncontrolled.close(); }
    // A new worker has no per-page map or cached transport keys. Let a real
    // update activate while this page stays mounted, then load media again.
    const workerFile = join(staticDist, 'relayServiceWorker.js');
    const workerSource = await readFile(workerFile, 'utf8');
    await writeFile(workerFile, workerSource + '\n// image pipeline worker update\n');
    try {
      await page.evaluate(async () => {
        const changed = new Promise<void>(done => navigator.serviceWorker.addEventListener('controllerchange', () => done(), { once: true }));
        await (await navigator.serviceWorker.ready).update();
        await changed;
      });
      const original = new URL((await image.getAttribute('src'))!);
      original.searchParams.delete('variant');
      const result = await page.evaluate(async url => {
        const response = await fetch(url, { credentials: 'include', cache: 'no-store' });
        const hash = await crypto.subtle.digest('SHA-256', await response.arrayBuffer());
        return { status: response.status, type: response.headers.get('content-type'), hash: Array.from(new Uint8Array(hash)).map(b => b.toString(16).padStart(2, '0')).join('') };
      }, original.href);
      facts.push({ event: 'after-worker-update', ...result });
      expect(result.status).toBe(200);
      expect(result.hash).toBe(createHash('sha256').update(await readFile(input)).digest('hex'));
    } finally { await writeFile(workerFile, workerSource); }
    await page.locator(`.matter-thread-row[href="/devices/${deviceId}/threads/${otherId}"]`).first().click();
    await expect(page).toHaveURL(new RegExp(`/threads/${otherId}$`));
    await page.locator(`.matter-thread-row[href="/devices/${deviceId}/threads/${threadId}"]`).first().click();
    await expect(page).toHaveURL(new RegExp(`/threads/${threadId}$`));
    const sameDocument = await page.evaluate(() => !!(window as unknown as { __imageFacts?: PipelineFact[] }).__imageFacts);
    expect(sameDocument).toBe(true);
    facts.push({ event: 'thread-switch', sameDocument });
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
    // Fail the actual endpoint by moving its real attachment; no intercepted
    // responses, dispatched image errors or synthetic transport failures.
    const source = new URL((await image.getAttribute('src'))!);
    const attachment = join(work, source.searchParams.get('path')!);
    await rename(attachment, attachment + '.held');
    try {
      await clearMediaCache();
      await page.reload();
      const mark = page.locator('[data-role="user"] .image-recovery-error .error-mark').last();
      await expect(mark).toBeVisible();
      expect(facts.some(f => f.event === 'response' && f.status === 400 && f.retry === true && f.worker === true)).toBe(true);
      if (process.env.E2E_EXPECT_DIAGNOSIS === '1') {
        await expect.poll(() => facts.filter(f => f.event === 'diagnosis').length).toBe(1);
        await mark.click();
        await expect(page.getByRole('dialog', { name: 'Why it failed' })).toContainText('File not found');
        await page.keyboard.press('Escape');
      }
      if (process.env.E2E_IMAGE_VISUALS === '1') {
        for (const theme of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) for (const device of ['desktop', 'phone']) {
          await page.setViewportSize(device === 'phone' ? { width: 390, height: 844 } : { width: 1100, height: 760 });
          await page.evaluate(({ theme, mode }) => { localStorage.setItem('pockymoe-theme-preset', theme); localStorage.setItem('remote-codex-theme-mode', mode); }, { theme, mode });
          await page.reload();
          await expect(page.locator('html')).toHaveAttribute('data-theme-preset', theme);
          await expect(mark).toBeVisible();
          await mark.click();
          await expect(page.getByRole('dialog', { name: 'Why it failed' })).toContainText('File not found');
          await page.screenshot({ path: `.scratch/chat-image-2/${theme}-${mode}-${device}.png` });
          await page.keyboard.press('Escape');
        }
        await page.setViewportSize({ width: 1100, height: 760 });
      }
    } finally { await rename(attachment + '.held', attachment); }
    await page.locator('[data-role="user"] .image-recovery-error .error-mark').last().click();
    await page.getByRole('button', { name: 'Retry', exact: true }).click();
    await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
    expect((await image.getAttribute('src'))!).toContain('_imageRetry=');
    expect(facts.some(f => f.event === 'response' && f.status === 200 && f.retry === true && f.worker === true)).toBe(true);
    // A damaged cached thumbnail is an actual image response that cannot decode;
    // the original endpoint still works and must rescue the tile.
    if (process.env.E2E_CORRUPT_THUMBNAIL !== '0') {
      const cache = join(root, 'cache/thread-thumbnails');
      const cached = (await readdir(cache)).filter(name => /\.(jpg|png)$/.test(name));
      expect(cached.length).toBeGreaterThan(0);
      await writeFile(join(cache, cached[0]!), 'damaged thumbnail cache');
      await clearMediaCache();
      await page.reload();
      await expect(image).toHaveAttribute('src', /_imageRetry=/);
      await expect.poll(() => image.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
      expect(new URL((await image.getAttribute('src'))!).searchParams.has('variant')).toBe(false);
      facts.push({ event: 'original-fallback', status: 200 });
    }
  } finally {
    facts.push(...await page.evaluate(() => (window as unknown as { __imageFacts?: PipelineFact[] }).__imageFacts ?? []));
    await mkdir('.scratch/chat-image-2', { recursive: true });
    await writeFile(`.scratch/chat-image-2/pipeline-${desktop ? 'electron' : 'chromium'}-${process.env.E2E_SUPERVISOR_LABEL ?? 'main'}.json`, JSON.stringify(facts, null, 2));
    console.log('Pipeline result:', JSON.stringify({ engine: desktop ? 'electron' : 'chromium', responses: facts.filter(f => f.event === 'response').length, diagnoses: facts.filter(f => f.event === 'diagnosis').length, originalFallback: facts.some(f => f.event === 'original-fallback') }));
  }
});
