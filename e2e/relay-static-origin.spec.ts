import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
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
const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==', 'base64');
const SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40"><rect width="120" height="40" fill="#264"/></svg>';
const processes: ChildProcess[] = [];
let root = '';
let pages: Server | undefined;
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
async function relayApi(path: string, method = 'GET', body?: unknown) {
  const session = (await context.cookies(relay)).find((cookie) => cookie.name === 'remote_codex_relay_session');
  const response = await fetch(relay + path, { method, headers: {
    origin: web, cookie: `remote_codex_relay_session=${session?.value ?? ''}`,
    ...(body === undefined ? {} : { 'content-type': 'application/json' }),
  }, ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(10_000) });
  const text = await response.text();
  return { status: response.status, data: text ? JSON.parse(text) : null };
}
// Headless Chromium has no PDF viewer; choose which browser kind to emulate.
const emulatePdfViewer = (enabled: boolean) => page.evaluate((value) =>
  Object.defineProperty(Navigator.prototype, 'pdfViewerEnabled', { configurable: true, get: () => value }), enabled);
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
  pages = await servePages(dist);
  context = await browser.newContext({ locale: 'en-US' });
  await context.addInitScript(() => localStorage.setItem('remote-codex.locale', 'en'));
  page = await context.newPage();
});

test.afterAll(async () => {
  await context?.close();
  await new Promise<void>((done) => (pages ? pages.close(() => done()) : done()));
  for (const child of processes.reverse()) {
    if (child.exitCode !== null) continue;
    const exited = new Promise<void>((done) => child.once('exit', () => done()));
    if (child.pid && process.platform !== 'win32') {
      try { process.kill(-child.pid, 'SIGTERM'); } catch { /* process group already exited */ }
    } else child.kill('SIGTERM');
    await exited;
  }
  if (root) await rm(root, { recursive: true, force: true });
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
  await expect(page.getByRole('heading', { name: 'Security', exact: true })).toBeVisible();
  const userId = (await relayApi('/relay/auth/session')).data.user.id;
  await context.addInitScript((id) => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`,
    JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })), userId);
  expect((await relayApi('/relay/devices', 'POST', { name: 'Cross-origin smoke device' })).status).toBe(200);
  await page.goto(`${web}/relay-devices`);
  await expect(page.getByText('Cross-origin smoke device', { exact: true }).first()).toBeVisible();
  const session = (await context.cookies(relay)).find((cookie) => cookie.name === 'remote_codex_relay_session');
  expect(Boolean(session?.httpOnly && session.secure && session.sameSite === 'Lax')).toBe(true);
  expect(apiRequests.length).toBeGreaterThan(2);
  expect(apiRequests.every((origin) => origin === relay)).toBe(true);
  expect(new URL(page.url()).origin).toBe(web);
});

test('Explorer media of a protected device loads through the encrypted transport on the static origin', async () => {
  test.setTimeout(180_000);
  const created = await relayApi('/relay/devices', 'POST', { name: 'Static media device' });
  expect(created.status).toBe(200);
  const deviceId: string = created.data.device.id;
  const devicePort = String(process.env.E2E_DEVICE_PORT ?? await freePort());
  const supervisor = start(resolve('target/debug/pockymoe'), ['relay-supervisor'], {
    HOST: '127.0.0.1', PORT: devicePort, POCKYMOE_RELAY_SUPERVISOR_PORT: devicePort, POCKYMOE_RELAY_SERVER_URL: relay, POCKYMOE_RELAY_AGENT_TOKEN: created.data.token,
    POCKYMOE_DATABASE_PATH: join(root, 'device.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces'),
    POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'), POCKYMOE_E2E_FAKE_RUNTIME: '1',
    POCKYMOE_ADMIN_USERNAME: 'device-admin', POCKYMOE_ADMIN_PASSWORD: randomBytes(24).toString('hex'),
  });
  const api = `/relay/devices/${deviceId}/api`;
  await expect.poll(async () => {
    if (supervisor.exitCode !== null) throw new Error(`Supervisor exited with ${supervisor.exitCode}`);
    return (await relayApi(`${api}/version`)).status;
  }, { timeout: 30_000 }).toBe(200);
  const work = join(root, 'workspace');
  const linked = join(root, 'linked');
  await mkdir(join(work, 'docs/images'), { recursive: true });
  await mkdir(linked);
  await writeFile(join(work, 'docs/images/plot.png'), PNG);
  await writeFile(join(work, 'chart.svg'), SVG);
  await writeFile(join(work, 'docs/REPORT.md'), '# Static report\n\n![Plot](images/plot.png)\n\n![Chart](../chart.svg)\n');
  const pdf = Buffer.from('%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n');
  await writeFile(join(work, 'docs/REPORT.pdf'), pdf);
  await writeFile(join(linked, 'shot.png'), PNG);
  await writeFile(join(linked, 'NOTES.md'), '# Linked notes\n\n![Linked shot](shot.png)\n');
  const workspace = (await relayApi(`${api}/workspaces`, 'POST', { label: 'Static media', absPath: work })).data;
  const started = (await relayApi(`${api}/threads/start`, 'POST', { workspaceId: workspace.id, model: 'fake', title: 'Static media thread' })).data;
  const threadId: string = started.thread?.id ?? started.id;
  // The fake runtime echoes the prompt, so the reply links the host file.
  const linkedPath = join(linked, 'NOTES.md');
  expect(linkedPath.length).toBeLessThan(76);
  expect((await relayApi(`${api}/threads/${threadId}/prompt`, 'POST', { prompt: linkedPath })).status).toBe(200);

  // Protect the device: it then refuses every plaintext relay request.
  await page.goto(`${web}/relay-devices`);
  await page.getByRole('button', { name: 'Protect all devices', exact: true }).click();
  await page.getByRole('dialog', { name: 'Protect all devices', exact: true }).getByRole('button', { name: 'Protect devices', exact: true }).click();
  // A fresh registration may still be inside its recent-unlock window.
  const unlock = page.getByRole('dialog', { name: /^(Unlock this browser|Confirm with your password)$/ });
  if (await unlock.waitFor({ timeout: 5_000 }).then(() => true, () => false)) {
    await unlock.getByLabel('Password', { exact: true }).fill(password);
    await unlock.getByRole('button', { name: 'Unlock', exact: true }).click();
  }
  const row = page.locator('article').filter({ has: page.getByText('Static media device', { exact: true }) });
  await expect(row.getByText('Protected', { exact: true })).toBeVisible({ timeout: 40_000 });
  const plaintext = await relayApi(`${api}/workspaces/${workspace.id}/files/raw?path=docs%2Fimages%2Fplot.png`);
  expect(plaintext).toMatchObject({ status: 403, data: { code: 'device_strict_mode' } });

  const consoleErrors: string[] = [];
  page.on('console', (message) => { if (message.type() === 'error' && !message.text().includes('status of 404')) consoleErrors.push(message.text()); });
  const media: { url: string; fromServiceWorker: boolean; status: number }[] = [];
  page.on('response', (response) => {
    if (/\/files\/(raw|download)|\/linked-files\/raw/.test(response.url()) && response.request().method() === 'GET')
      media.push({ url: response.url(), fromServiceWorker: response.fromServiceWorker(), status: response.status() });
  });
  await page.goto(`${web}/devices/${deviceId}/threads/${threadId}`);
  await emulatePdfViewer(true);
  await page.locator('.matter-rail').getByRole('button', { name: 'Toggle Explorer', exact: true }).click();
  const files = page.getByTestId('workspace-panel');
  await files.getByRole('treeitem', { name: 'docs', exact: true }).getByRole('button', { name: 'docs', exact: true }).dblclick();
  await files.getByRole('treeitem', { name: 'REPORT.md', exact: true }).getByRole('button', { name: 'REPORT.md', exact: true }).click();
  const markdown = files.locator('.thread-graph-markdown-preview');
  await expect(markdown.getByRole('heading', { name: 'Static report', exact: true })).toBeVisible();
  for (const [name, width] of [['Plot', 1], ['Chart', 120]] as const) {
    const image = markdown.getByRole('img', { name, exact: true }).or(markdown.locator(`img[alt="${name}"]`)).first();
    await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.complete ? element.naturalWidth : -1), { message: `${name} image decoded` }).toBe(width);
    expect(new URL((await image.getAttribute('src'))!).origin).toBe(relay);
  }
  expect(media.filter((entry) => entry.url.includes('/files/raw?')).every((entry) => entry.fromServiceWorker && entry.status === 200)).toBe(true);

  // Desktop: the PDF bytes come through the encrypted API into a blob: frame.
  const download = page.waitForResponse((response) => response.url().includes('/files/download') && response.request().method() === 'GET');
  await files.getByRole('treeitem', { name: 'REPORT.pdf', exact: true }).getByRole('button', { name: 'REPORT.pdf', exact: true }).click();
  const downloaded = await download;
  expect(downloaded.headers()['x-rcd-encrypted']).toBe('1');
  const frame = files.locator('iframe[title^="PDF preview"]');
  await expect(frame).toHaveAttribute('src', /^blob:http:\/\/localhost:\d+\//);
  expect(new URL((await frame.getAttribute('src'))!.slice(5)).origin).toBe(web);
  // The headless shell has no PDF plugin, so only check that the frame is not
  // blocked (a refused frame becomes chrome-error://, as in the bug report).
  await page.waitForTimeout(500);
  expect(page.frames().map((candidate) => candidate.url()).filter((url) => url.startsWith('chrome-error:'))).toEqual([]);

  // Phones (no viewer in frames): compact Open/Download instead of a blank frame.
  await emulatePdfViewer(false);
  await files.getByRole('treeitem', { name: 'REPORT.md', exact: true }).getByRole('button', { name: 'REPORT.md', exact: true }).click();
  await expect(markdown).toBeVisible();
  await files.getByRole('treeitem', { name: 'REPORT.pdf', exact: true }).getByRole('button', { name: 'REPORT.pdf', exact: true }).click();
  const actions = files.getByTestId('workspace-pdf-actions');
  await expect(actions).toContainText('This browser cannot show a PDF inside the page.');
  await expect(files.locator('iframe[title^="PDF preview"]')).toHaveCount(0);
  await expect(actions.getByRole('link', { name: 'Open REPORT.pdf in a new tab', exact: true })).toHaveAttribute('href', /^blob:/);
  const saved = page.waitForEvent('download');
  await actions.getByRole('link', { name: 'Download REPORT.pdf', exact: true }).click();
  const file = await saved;
  expect(file.suggestedFilename()).toBe('REPORT.pdf');
  expect((await readFile((await file.path())!)).subarray(0, 5).toString()).toBe('%PDF-');

  // A linked host Markdown file resolves its relative images beside itself.
  await page.locator('main button, main a').filter({ hasText: linkedPath }).last().click();
  await expect(markdown.getByRole('heading', { name: 'Linked notes', exact: true })).toBeVisible();
  const shot = markdown.locator('img[alt="Linked shot"]');
  await expect.poll(() => shot.evaluate((element: HTMLImageElement) => element.complete ? element.naturalWidth : -1)).toBe(1);
  expect(new URL((await shot.getAttribute('src'))!).searchParams.get('path')).toBe(join(linked, 'shot.png'));
  await expect(files.getByText('Linked file outside the workspace: read-only preview.', { exact: true })).toBeVisible();
  expect(media.filter((entry) => entry.url.includes('/linked-files/raw')).every((entry) => entry.fromServiceWorker && entry.status === 200)).toBe(true);
  expect(consoleErrors.filter((text) => /Content Security Policy|frame/i.test(text))).toEqual([]);
});
