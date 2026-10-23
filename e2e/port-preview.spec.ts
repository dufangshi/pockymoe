import { promisify } from 'node:util';
import { test, expect } from '@playwright/test';
import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { createServer, request as httpRequest } from 'node:http';
import { Readable } from 'node:stream';
import { WebSocketServer } from 'ws';

test.use({ actionTimeout: 15_000 });
test('private device ports support browser links, HTTP streaming, WebSocket and immediate revocation', async ({ page, context }, testInfo) => {
  const root = await mkdtemp(resolve('.local/port-preview-'));
  const processes: ChildProcess[] = [];
  const logs: string[] = [];
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const password = randomBytes(24).toString('hex');
  const freePort = async () => {
    const server = createServer();
    await new Promise<void>(done => server.listen(0, '127.0.0.1', done));
    const port = (server.address() as { port: number }).port;
    await new Promise<void>(done => server.close(() => done()));
    return port;
  };
  const rp = await freePort(), sp = await freePort();
  const base = `http://127.0.0.1:${rp}`;
  const binaries = resolve(process.env.E2E_SECURITY_BINARY ?? 'target/debug/pockymoe');
  function start(command: string, extra: Record<string, string>) {
    const proc = spawn(binaries, [command], {
      env: { ...env, HOST: '127.0.0.1', POCKYMOE_E2E_FAKE_RUNTIME: '1',
        RUST_LOG: 'info,pockymoe_relay::preview=debug,pockymoe_supervisor::ports=debug',
        POCKYMOE_ADMIN_USERNAME: 'admin', POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), ...extra },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    for (const output of [proc.stdout, proc.stderr]) output!.on('data', data => logs.push(String(data)));
    processes.push(proc); return proc;
  }
  async function api(path: string, method = 'GET', data?: unknown, token?: string) {
    const response = await fetch(base + path, { method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(data ? { 'content-type': 'application/json' } : {}) },
      ...(data ? { body: JSON.stringify(data) } : {}), signal: AbortSignal.timeout(10_000) });
    const text = await response.text();
    return { status: response.status, data: text ? JSON.parse(text) : null };
  }
  async function account(username: string) {
    expect((await api('/relay/auth/register', 'POST', { username, email: `${username}@example.test`, password })).status).toBe(200);
    return (await api('/relay/auth/login', 'POST', { username, password })).data.token as string;
  }
  let streamClosed = false;
  let observedCookies = '';
  const service = createServer(async (req, res) => {
    observedCookies = req.headers.cookie ?? '';
    if (req.url === '/blocked') { res.writeHead(403); res.end('Blocked request. This host is not allowed.'); return; }
    if (req.url === '/forbidden') { res.writeHead(403); res.end('Login required'); return; }
    if (req.url === '/sse') {
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.write('data: immediate\n\n');
      req.on('close', () => { streamClosed = true; });
      return;
    }
    if (req.url === '/echo') {
      const parts: Buffer[] = []; for await (const part of req) parts.push(part);
      res.writeHead(201, { 'content-type': 'application/octet-stream' }); res.end(Buffer.concat(parts)); return;
    }
    if (req.url === '/redirect') {
      res.writeHead(302, { location: `http://localhost:${(service.address() as { port: number }).port}/app?q=1` }); res.end(); return;
    }
    if (req.url === '/asset.bin') { res.end(Buffer.from([0, 255, 12, 0, 128])); return; }
    res.writeHead(200, { 'content-type': 'text/html', 'set-cookie': 'app=value; Domain=localhost; Path=/; HttpOnly' });
    res.end(`<!doctype html><html><body><h1>Device web preview</h1><p id="ws">Connecting</p><script>
      const socket=new WebSocket('ws://'+location.host+'/ws');
      socket.onopen=()=>socket.send('hot reload');
      socket.onmessage=e=>document.querySelector('#ws').textContent=e.data;
      socket.onclose=()=>document.querySelector('#ws').textContent='Disconnected';
    </script></body></html>`);
  });
  const wsServer = new WebSocketServer({ server: service, path: '/ws' });
  wsServer.on('connection', socket => socket.on('message', data => socket.send(data.toString())));
  const port = await freePort();
  async function cli(...args: string[]) {
    const { stdout } = await promisify(execFile)(binaries, ['--cli-config', join(root, 'device.cli.json'), 'preview', ...args], { env, timeout: 20_000 });
    return JSON.parse(stdout);
  }
  // Native fetch in Node 24 ignores an explicitly supplied Host header. Use
  // node:http to reach the isolated listener with the actual preview authority.
  const fetchPreview = (url: URL, cookie?: string, init: RequestInit = {}) => new Promise<Response>((done, reject) => {
    const req = httpRequest(base + url.pathname + url.search, {
      method: init.method ?? 'GET', headers: { host: url.host, ...(cookie ? { cookie } : {}), ...init.headers },
      signal: init.signal ?? AbortSignal.timeout(10_000),
    }, res => {
      const headers = new Headers();
      for (const [name, value] of Object.entries(res.headers)) {
        for (const item of Array.isArray(value) ? value : [value]) if (item !== undefined) headers.append(name, item);
      }
      done(new Response(Readable.toWeb(res) as ReadableStream<Uint8Array>, { status: res.statusCode!, headers }));
    });
    req.on('error', reject); req.end(init.body);
  });
  try {
    start('relay', { PORT: String(rp), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
      POCKYMOE_PUBLIC_BASE_URL: base, POCKYMOE_PORT_PREVIEW_BASE_URL: `http://preview.localhost:${rp}` });
    await expect.poll(() => api('/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    const owner = await account('owner'), other = await account('other');
    // This spec shares through the relay API and tests access, not the
    // safety-code check (workspace-sharing.spec.ts covers it): turn it off.
    expect((await api('/relay/account/preferences', 'PATCH', { requireSafetyCode: false }, owner)).status).toBe(200);
    const created = await api('/relay/devices', 'POST', { name: 'Port preview device' }, owner);
    expect(created.status).toBe(200);
    const deviceId = created.data.device.id;
    start('relay-supervisor', { PORT: String(sp), POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: base, POCKYMOE_RELAY_AGENT_TOKEN: created.data.token,
      POCKYMOE_DATABASE_PATH: join(root, 'device.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces') });
    await expect.poll(() => api('/healthz').then(r => r.data.connectedSupervisors)).toBe(1);
    const deviceApi = `/relay/devices/${deviceId}/api`;
    // Real local CLI reserves a stable address while nothing is listening yet.
    const reserved = await cli('create', '--port', String(port), '--label', 'Test website');
    expect(reserved.hostname).toBe(`p-${reserved.id}.preview.localhost`);
    expect(reserved.url).not.toContain('__rc_launch');
    expect((await cli('create', '--port', String(port))).id).toBe(reserved.id);
    expect((await cli('check', String(port))).check.status).toBe('serviceNotRunning');
    await new Promise<void>(done => service.listen(port, '127.0.0.1', done));
    const ready = await cli('check', String(port), '--websocket-path', '/ws');
    expect(ready.check.status).toBe('httpReady');
    expect(ready.check.websocket).toBe('handshakeAccepted');
    expect((await cli('check', String(port), '--path', '/blocked')).check.status).toBe('hostRejected');
    expect((await cli('check', String(port), '--path', '/forbidden')).check.status).toBe('httpError');
    expect((await cli('check', String(port), '--websocket-path', '/missing-ws')).check.websocket).toBe('handshakeFailed');
    expect((await cli('list')).mappings[0].id).toBe(reserved.id);
    const work = join(root, 'workspace'); await mkdir(work);
    const workspace = await api(deviceApi + '/workspaces', 'POST', { absPath: work, label: 'Preview' }, owner);
    expect(workspace.status).toBe(200);
    const thread = await api(deviceApi + '/threads/start', 'POST', { workspaceId: workspace.data.id, model: 'fake', title: 'Port preview' }, owner);
    expect(thread.status).toBe(200);
    const tid = thread.data.thread?.id ?? thread.data.id;
    await context.addCookies([{ name: 'remote_codex_relay_session', value: owner, url: base }]);
    await page.goto(`${base}/devices/${deviceId}/threads/${tid}`);
    await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
    const managerButton = page.getByRole('button', { name: 'Port mappings', exact: true });
    await managerButton.click();
    const manager = page.getByRole('dialog', { name: 'Port mappings', exact: true });
    await expect(manager.getByRole('button', { name: 'Enable', exact: true })).toBeEnabled();
    const bounds = (await manager.boundingBox())!;
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    await manager.getByRole('spinbutton', { name: 'HTTP port' }).fill(String(port));
    await manager.getByRole('textbox', { name: 'Port label' }).fill('Test website');
    const enableResponse = page.waitForResponse(r => r.url().includes('/api/port-mappings') && r.request().method() === 'POST');
    await manager.getByRole('button', { name: 'Enable', exact: true }).click();
    expect((await enableResponse).headers()['x-rcd-encrypted']).toBeTruthy();
    await expect(manager.getByText(`Test website · 127.0.0.1:${port}`)).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('port-mappings-manager.png') });
    const mapping = (await api(deviceApi + '/port-mappings', 'GET', undefined, owner)).data.mappings[0];
    // Idempotent enabling, validation and owner-only control.
    expect((await api(deviceApi + '/port-mappings', 'POST', { port }, owner)).data.id).toBe(mapping.id);
    expect((await api(deviceApi + '/port-mappings', 'POST', { port: sp }, owner)).status).toBe(400);
    expect((await api(deviceApi + '/port-mappings', 'POST', { port: 0 }, owner)).status).toBe(400);
    expect((await api(deviceApi + '/port-mappings', 'POST', { port }, other)).status).toBe(401);
    expect((await api('/relay/grants', 'POST', { deviceId, targetIdentifier: 'other', scope: 'device', threadAccess: 'control', workspaceAccess: 'write', canCreateThreads: true }, owner)).status).toBe(200);
    expect((await api(deviceApi + '/port-mappings', 'POST', { port }, other)).status).toBe(403);
    const openPath = `/relay/devices/${deviceId}/port-mappings/${mapping.id}/open`;
    expect((await api(openPath, 'POST', { path: '/' }, other)).status).toBe(403);
    expect((await api(openPath, 'POST', { path: '//evil.test' }, owner)).status).toBe(400);
    // Real browser navigation through the public-style distinct origin, including WS.
    const direct = await context.newPage();
    await direct.goto(reserved.url + 'app?q=1');
    await expect(direct.getByRole('heading', { name: 'Device web preview' })).toBeVisible();
    await expect(direct.locator('#ws')).toHaveText('hot reload');
    expect(new URL(direct.url()).pathname).toBe('/app');
    expect(new URL(direct.url()).search).toBe('?q=1');
    await direct.close();
    // A fresh browser must enter sign-in; a different owner cannot redeem the link.
    const fresh = await context.browser()!.newContext();
    try {
      const anonymous = await fresh.newPage();
      await anonymous.goto(reserved.url);
      await expect(anonymous).toHaveURL(/\/relay-portal\?returnTo=/);
      expect(decodeURIComponent(new URL(anonymous.url()).searchParams.get('returnTo')!)).toContain(`/devices/${deviceId}/ports/${reserved.id}`);
      await anonymous.getByRole('textbox', { name: 'Email or username', exact: true }).fill('owner');
      await anonymous.getByLabel('Password', { exact: true }).fill(password);
      await anonymous.getByRole('button', { name: 'Sign in', exact: true }).click();
      await expect(anonymous.getByRole('heading', { name: 'Device web preview' })).toBeVisible();
      await expect(anonymous.locator('#ws')).toHaveText('hot reload');
      await fresh.clearCookies();
      await fresh.addCookies([{ name: 'remote_codex_relay_session', value: other, url: base }]);
      await anonymous.goto(reserved.openUrl);
      await expect(anonymous.getByRole('alert')).toBeVisible();
      expect(new URL(anonymous.url()).host).toBe(new URL(base).host);
    } finally { await fresh.close(); }
    const popupPromise = page.waitForEvent('popup');
    await manager.getByRole('button', { name: 'Open', exact: true }).click();
    const preview = await popupPromise;
    await expect(preview.getByRole('heading', { name: 'Device web preview' })).toBeVisible();
    await expect(preview.locator('#ws')).toHaveText('hot reload');
    expect(new URL(preview.url()).hostname).toBe(`p-${mapping.id}.preview.localhost`);
    expect(new URL(preview.url()).searchParams.has('__rc_launch')).toBe(false);
    const browserCookie = (await context.cookies(preview.url())).find(c => c.name === 'rc_port_preview')!;
    expect(browserCookie.httpOnly).toBe(true);
    const url = new URL(preview.url());
    const cookie = `rc_port_preview=${browserCookie.value}; remote_codex_relay_session=NEVER_FORWARD; app=value`;
    expect((await fetchPreview(url)).status).toBe(401);
    expect((await fetchPreview(url, undefined, { method: 'POST', headers: { accept: 'text/html', 'sec-fetch-mode': 'navigate' } })).status).toBe(401);
    const payload = randomBytes(512 * 1024 + 37);
    url.pathname = '/echo';
    const echo = await fetchPreview(url, cookie, { method: 'POST', body: payload });
    expect(echo.status).toBe(201); expect(Buffer.from(await echo.arrayBuffer()).equals(payload)).toBe(true);
    expect(observedCookies).toBe('app=value');
    url.pathname = '/asset.bin';
    // Short responses can finish before the WS handshake/control frame settles;
    // exercise that ordering repeatedly without retries masking lost bytes.
    for (let n = 0; n < 20; n++) {
      const asset = await fetchPreview(url, cookie);
      const bytes = Buffer.from(await asset.arrayBuffer());
      expect(asset.status, bytes.toString()).toBe(200);
      expect(bytes).toEqual(Buffer.from([0, 255, 12, 0, 128]));
    }
    url.pathname = '/redirect';
    expect((await fetchPreview(url, cookie)).headers.get('location')).toBe(`${url.origin}/app?q=1`);
    url.pathname = '/echo';
    expect((await fetchPreview(url, cookie, { method: 'POST', headers: { origin: 'https://evil.test' } })).status).toBe(403);
    url.pathname = '/sse';
    const abort = new AbortController();
    const streaming = await fetchPreview(url, cookie, { signal: abort.signal });
    const reader = streaming.body!.getReader();
    expect(new TextDecoder().decode((await reader.read()).value)).toContain('data: immediate');
    abort.abort(); await expect.poll(() => streamClosed).toBe(true);
    // A launch ticket is single-use and cannot authorize another mapping/owner.
    const handoff = new URL((await api(openPath, 'POST', { path: '/' }, owner)).data.url);
    expect((await fetchPreview(handoff)).status).toBe(303);
    expect((await fetchPreview(handoff)).status).toBe(401);
    await manager.getByRole('button', { name: 'Stop', exact: true }).click();
    await expect(manager.getByText('No enabled ports.')).toBeVisible();
    await expect(preview.locator('#ws')).toHaveText('Disconnected');
    url.pathname = '/'; expect((await fetchPreview(url, cookie)).status).toBe(404);
    // A chat-localhost link prompts; cancelling cannot re-enable the stopped port.
    await manager.getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Thread tools', exact: true }).click();
    await expect(managerButton).not.toBeVisible();
    await page.locator('.thread-graph-scroll-content').evaluate((node, port) => {
      const link = document.createElement('a'); link.href = `http://localhost:${port}/app?q=1#section`;
      link.textContent = 'Open local test app'; node.prepend(link);
    }, port);
    await page.getByRole('link', { name: 'Open local test app' }).click();
    const confirm = page.getByRole('dialog', { name: 'Open device web service?' });
    await expect(confirm).toBeVisible();
    await confirm.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect((await api(deviceApi + '/port-mappings', 'GET', undefined, owner)).data.mappings).toEqual([]);
    await page.getByRole('link', { name: 'Open local test app' }).click();
    await expect(confirm.getByRole('button', { name: 'Enable and open' })).toBeEnabled();
    const linkedPromise = page.waitForEvent('popup');
    await confirm.getByRole('button', { name: 'Enable and open' }).click();
    const linked = await linkedPromise;
    await expect(linked.getByRole('heading', { name: 'Device web preview' })).toBeVisible();
    await expect(linked.locator('#ws')).toHaveText('hot reload');
    expect(new URL(linked.url()).pathname + new URL(linked.url()).search + new URL(linked.url()).hash).toBe('/app?q=1#section');
    expect((await fetchPreview(new URL(linked.url()), cookie)).status).toBe(401);
    const newCookie = (await context.cookies(linked.url())).find(c => c.name === 'rc_port_preview')!;
    expect((await api('/relay/auth/logout', 'POST', {}, owner)).status).toBe(200);
    expect((await fetchPreview(new URL(linked.url()), `rc_port_preview=${newCookie.value}`)).status).toBe(401);
    await expect(linked.locator('#ws')).toHaveText('Disconnected');
    await linked.reload();
    await expect(linked).toHaveURL(/\/relay-portal\?returnTo=/);
    expect((await cli('stop', String(port))).stopped).toBe(true);
    expect((await cli('list')).mappings).toEqual([]);
    await page.screenshot({ path: testInfo.outputPath('port-preview-toolbar.png') });
    await linked.close(); await preview.close();
  } finally {
    await testInfo.attach('isolated-services.log', { body: logs.join(''), contentType: 'text/plain' });
    await Promise.all(processes.map(proc => new Promise<void>(done => {
      if (proc.exitCode !== null || proc.signalCode !== null) return done();
      proc.once('exit', () => done()); proc.kill('SIGTERM');
      setTimeout(() => proc.kill('SIGKILL'), 2000).unref();
    })));
    for (const client of wsServer.clients) client.terminate();
    wsServer.close(); service.closeAllConnections();
    await new Promise<void>(done => service.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
});
