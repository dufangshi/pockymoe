import { test, expect, type Page } from '@playwright/test';
import { spawn, execFile, type ChildProcess } from 'node:child_process';
import { promisify } from 'node:util';
import { createServer as httpServer } from 'node:http';
import { createServer as tcpServer, connect, type Socket } from 'node:net';
import { randomBytes, createHash } from 'node:crypto';
import { readFile, mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { WebSocketServer } from 'ws';
import { readRecoveryCode, confirmRecoveryCode } from '../e2e/helpers/account-key-fixture';

// All accounts, mappings, cookies and launch URLs stay in memory or the private
// disposable directory. Evidence contains timing/status/path categories only.
test('isolated private preview authorization and RTT benchmark', async ({ browser }) => {
  const evidence = resolve(process.env.E2E_PREVIEW_BENCH_OUTPUT ?? '.scratch/port-bench');
  await mkdir(evidence, { recursive: true, mode: 0o700 });
  const root = await mkdtemp(join(evidence, 'private-'));
  const binary = resolve(process.env.E2E_SECURITY_BINARY ?? 'target/debug/pockymoe');
  const binarySha256 = createHash('sha256').update(await readFile(binary)).digest('hex');
  const webDist = resolve(process.env.E2E_PREVIEW_WEB_DIST ?? 'apps/supervisor-web/dist');
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key)));
  const password = randomBytes(24).toString('hex');
  const processes: ChildProcess[] = [];
  const sockets = new Set<Socket>();
  const pendingTimers = new Set<NodeJS.Timeout>();
  const rows: unknown[] = [];
  const network: unknown[] = [];
  const contexts: Awaited<ReturnType<typeof browser.newContext>>[] = [];
  const accountContexts = new Map<string, Awaited<ReturnType<typeof browser.newContext>>>();
  let oneWayMs = 0, dataConnections = 0, upstreamRequests = 0;
  const upstreamCounts: Record<string, number> = {};
  const freePort = async () => {
    const server = tcpServer();
    await new Promise<void>(done => server.listen(0, '127.0.0.1', done));
    const port = (server.address() as { port: number }).port;
    await new Promise<void>(done => server.close(() => done()));
    return port;
  };
  const rp = await freePort(), pp = await freePort(), sp = await freePort();
  const port = Number(process.env.E2E_PREVIEW_FIXTURE_PORT ?? 19473);
  // Chromium accepts the Relay's Secure account cookie on localhost.
  const base = `http://localhost:${rp}`;
  const proxyBase = `http://127.0.0.1:${pp}`;
  const directBase = `http://127.0.0.1:${port}`;
  const proxy = tcpServer(client => {
    const target = connect(rp, '127.0.0.1');
    let first = true;
    for (const socket of [client, target]) {
      sockets.add(socket); socket.setNoDelay(true);
      socket.on('close', () => sockets.delete(socket));
      socket.on('error', () => { client.destroy(); target.destroy(); });
    }
    const forward = (from: Socket, to: Socket, inspect = false) => from.on('data', data => {
      if (inspect && first) {
        first = false;
        if (data.subarray(0, 64).toString().startsWith('GET /supervisor/port-stream/')) dataConnections++;
      }
      // Delay bytes in each direction, preserving packet order. TCP establishment
      // remains loopback: this models application RTT, not WAN TCP/TLS setup.
      if (!oneWayMs) { if (!to.destroyed) to.write(data); return; }
      const timer = setTimeout(() => {
        pendingTimers.delete(timer); if (!to.destroyed) to.write(data);
      }, oneWayMs);
      pendingTimers.add(timer);
    });
    forward(client, target, true); forward(target, client);
    const endAfterBytes = (socket: Socket) => {
      if (!oneWayMs) { socket.end(); return; }
      const timer = setTimeout(() => { pendingTimers.delete(timer); if (!socket.destroyed) socket.end(); }, oneWayMs);
      pendingTimers.add(timer);
    };
    client.on('end', () => endAfterBytes(target)); target.on('end', () => endAfterBytes(client));
  });
  const service = httpServer((req, res) => {
    upstreamRequests++;
    const path = req.url!.split('?')[0];
    upstreamCounts[path] = (upstreamCounts[path] ?? 0) + 1;
    res.setHeader('cache-control', 'no-store');
    if (path === '/sse') {
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      let n = 0; res.write(`data: ${n++}\n\n`);
      const timer = setInterval(() => {
        res.write(`data: ${n++}\n\n`); if (n === 5) { clearInterval(timer); res.end(); }
      }, 60);
      req.on('close', () => clearInterval(timer)); return;
    }
    if (path === '/small') { res.setHeader('content-type', 'application/octet-stream'); res.end(Buffer.alloc(1024, 65)); return; }
    if (path === '/large') { res.setHeader('content-type', 'application/octet-stream'); res.end(Buffer.alloc(1024 * 1024, 65)); return; }
    if (path.startsWith('/module/')) {
      const n = Number(path.split('/')[2]);
      res.setHeader('content-type', 'application/javascript');
      res.end(n < 4 ? `import '/module/${n + 1}'; window.modules=(window.modules||0)+1;` : 'window.modules=(window.modules||0)+1;'); return;
    }
    if (path.startsWith('/asset/')) { res.setHeader('content-type', 'application/javascript'); res.end('window.assets=(window.assets||0)+1;'); return; }
    res.setHeader('content-type', 'text/html');
    res.end(`<!doctype html><html><body><h1>Isolated preview benchmark</h1>
      <button id="interact">Fetch</button><output id="result"></output>
      <script>document.querySelector('#interact').onclick=async()=>{const t=performance.now();await(await fetch('/small')).arrayBuffer();window.clickMs=performance.now()-t;document.querySelector('#result').textContent='ready';};</script>
      ${path === '/assets' ? Array.from({ length: 12 }, (_, n) => `<script defer src="/asset/${n}"></script>`).join('') : ''}
      ${path === '/modules' ? '<script type="module" src="/module/0"></script>' : ''}
      </body></html>`);
  });
  const wsServer = new WebSocketServer({ server: service, path: '/ws' });
  wsServer.on('connection', socket => socket.on('message', data => socket.send(data.toString())));
  const context = async () => { const c = await browser.newContext(); await c.grantPermissions(['local-network-access'], { origin: base }); contexts.push(c); return c; };
  const category = (raw: string) => {
    const u = new URL(raw);
    if (u.hostname.startsWith('p-')) return 'preview:' + u.pathname;
    return 'relay:' + u.pathname.replace(/\/devices\/[^/]+/g, '/devices/[id]').replace(/\/ports\/[^/]+/g, '/ports/[id]').replace(/\/port-mappings\/[^/]+/g, '/port-mappings/[id]');
  };
  const instrument = (page: Page, phase: string) => {
    page.on('requestfinished', async request => {
      network.push({ phase, route: category(request.url()), method: request.method(), timing: request.timing(), status: (await request.response())?.status() });
    });
    page.on('requestfailed', request => network.push({ phase, route: category(request.url()), failed: true }));
  };
  function start(command: string, extra: Record<string, string>) {
    const proc = spawn(binary, [command], {
      env: { ...env, HOST: '127.0.0.1', POCKYMOE_E2E_FAKE_RUNTIME: '1', RUST_LOG: 'warn',
        POCKYMOE_ADMIN_USERNAME: 'admin', POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'), ...extra },
      stdio: 'ignore',
    }); processes.push(proc);
  }
  async function api(path: string, method = 'GET', data?: unknown, token?: string) {
    const response = await fetch(base + path, { method,
      headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), ...(data ? { 'content-type': 'application/json' } : {}) },
      ...(data ? { body: JSON.stringify(data) } : {}), signal: AbortSignal.timeout(10_000) });
    const text = await response.text();
    let parsed; try { parsed = text ? JSON.parse(text) : null; } catch { parsed = null; }
    return { status: response.status, data: parsed };
  }
  async function account(username: string) {
    const c = await context(); const page = await c.newPage();
    instrument(page, 'registration');
    await page.goto(base + '/relay-portal?returnTo=%2Frelay-account');
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await page.getByLabel('Email', { exact: true }).fill(`${username}@example.test`);
    await page.getByLabel('Username', { exact: true }).fill(username);
    await page.getByLabel('Password', { exact: true }).fill(password);
    await page.getByLabel('Confirm password', { exact: true }).fill(password);
    await page.locator('form button[type="submit"]').click();
    const recovery = await readRecoveryCode(page);
    await confirmRecoveryCode(page, recovery);
    await expect.poll(() => page.getByRole('heading', { name: 'Security', exact: true }).isVisible()).toBe(true);
    const cookie = (await c.cookies(base)).find(c => c.name === 'remote_codex_relay_session');
    expect(!!cookie).toBe(true); accountContexts.set(cookie!.value, c); return cookie!.value;
  }
  async function cli(...args: string[]) {
    const { stdout } = await promisify(execFile)(binary, ['--cli-config', join(root, 'device.cli.json'), 'preview', ...args], { env, timeout: 20_000 });
    return JSON.parse(stdout);
  }
  async function navigate(page: Page, url: string) {
    const started = performance.now(); await page.goto(url);
    await expect.poll(() => page.getByRole('heading', { name: 'Isolated preview benchmark' }).isVisible()).toBe(true);
    return performance.now() - started;
  }
  async function interactions(page: Page) {
    return page.evaluate(async () => {
      let requestNumber = 0;
      // Distinct URLs avoid Chromium's cache lock serializing identical in-flight
      // fetches, which otherwise disguises transport concurrency.
      const request = async () => { const t = performance.now(); const r = await fetch('/small?request=' + requestNumber++, { cache: 'no-store' }); const bytes = (await r.arrayBuffer()).byteLength;
        if (r.status !== 200 || bytes !== 1024) throw new Error('Fixture fetch failed'); return performance.now() - t; };
      const serial: number[] = []; for (let i = 0; i < 8; i++) serial.push(await request());
      const parallelStart = performance.now(); const parallel = await Promise.all(Array.from({ length: 8 }, request));
      const parallelWall = performance.now() - parallelStart;
      const parallel20Start = performance.now(); await Promise.all(Array.from({ length: 20 }, request)); const parallel20Wall = performance.now() - parallel20Start;
      const largeStart = performance.now(); const largeResponse = await fetch('/large');
      if (largeResponse.status !== 200 || (await largeResponse.arrayBuffer()).byteLength !== 1024 * 1024) throw new Error('Large fixture fetch failed');
      const download1MiBMs = performance.now() - largeStart;
      const wsStart = performance.now(); const ws = new WebSocket('ws://' + location.host + '/ws');
      await new Promise<void>((done, reject) => { ws.onopen = () => done(); ws.onerror = () => reject(new Error('WS failed')); });
      const wsOpenMs = performance.now() - wsStart;
      const echoes: number[] = [];
      for (let i = 0; i < 5; i++) { const t = performance.now(); await new Promise<void>(done => { ws.onmessage = e => { if (e.data === 'echo') done(); }; ws.send('echo'); }); echoes.push(performance.now() - t); }
      ws.close();
      const sseStart = performance.now(); const response = await fetch('/sse'); const reader = response.body!.getReader();
      const events: number[] = []; let buffer = '';
      while (true) { const { value, done } = await reader.read(); if (done) break; buffer += new TextDecoder().decode(value);
        while (buffer.includes('\n\n')) { events.push(performance.now() - sseStart); buffer = buffer.slice(buffer.indexOf('\n\n') + 2); } }
      if (events.length !== 5) throw new Error('SSE event loss');
      document.querySelector<HTMLButtonElement>('#interact')!.click();
      await new Promise<void>(done => { const observer = new MutationObserver(() => { observer.disconnect(); done(); }); observer.observe(document.querySelector('#result')!, { childList: true }); });
      return { serial, serialWall: serial.reduce((a, b) => a + b, 0), parallel, parallelWall, parallel20Wall, download1MiBMs, wsOpenMs, wsEchoMs: echoes, sseArrivalMs: events, clickFetchMs: (window as unknown as { clickMs: number }).clickMs };
    });
  }
  try {
    start('relay', { PORT: String(rp), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true', POCKYMOE_RELAY_WEB_DIST_DIR: webDist,
      POCKYMOE_PUBLIC_BASE_URL: base, POCKYMOE_PORT_PREVIEW_BASE_URL: `http://preview.localhost:${rp}` });
    await expect.poll(() => api('/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    await new Promise<void>(done => proxy.listen(pp, '127.0.0.1', done));
    const owner = await account('bench-owner'), other = await account('bench-other');
    const created = await api('/relay/devices', 'POST', { name: 'Disposable benchmark' }, owner);
    expect(created.status).toBe(200); const deviceId = created.data.device.id;
    start('relay-supervisor', { PORT: String(sp), POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: proxyBase, POCKYMOE_RELAY_AGENT_TOKEN: created.data.token,
      POCKYMOE_DATABASE_PATH: join(root, 'device.sqlite'), POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces') });
    await expect.poll(() => api('/healthz').then(r => r.data.connectedSupervisors)).toBe(1);
    // Reserve only on the disposable Supervisor, before binding the upstream.
    const mapping = await cli('create', '--port', String(port), '--label', 'Disposable benchmark fixture');
    await new Promise<void>(done => service.listen(port, '127.0.0.1', done));
    const stableUrl = mapping.url;
    const ownerContext = await context(); const page = await ownerContext.newPage(); instrument(page, 'cold-login');
    // The CLI's locally issued lease is a supported owner authorization path,
    // needed because this disposable device has not been account-key bound.
    const loginStart = performance.now(); await page.goto(mapping.openUrl);
    await expect.poll(() => new URL(page.url()).pathname === '/relay-portal').toBe(true);
    const loginPageMs = performance.now() - loginStart;
    const beforeUnauthorized = upstreamRequests;
    const unauth = await ownerContext.request.get(stableUrl, { maxRedirects: 0 }); expect(unauth.status()).toBe(401);
    expect(upstreamRequests).toBe(beforeUnauthorized);
    await page.getByRole('textbox', { name: 'Email or username', exact: true }).fill('bench-owner');
    await page.getByLabel('Password', { exact: true }).fill(password);
    const submitStart = performance.now(); await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await expect.poll(() => page.getByRole('heading', { name: 'Isolated preview benchmark' }).isVisible()).toBe(true);
    const submitToPreviewMs = performance.now() - submitStart;
    rows.push({ phase: 'authorization', mode: 'real account login plus CLI-issued local preview lease', loginPageMs, submitToPreviewMs, anonymousApiStatus: unauth.status() });
    const alienContext = accountContexts.get(other)!;
    const alien = await alienContext.newPage();
    const beforeAlien = upstreamRequests;
    await alien.goto(mapping.openUrl); await expect.poll(() => alien.getByRole('alert').isVisible()).toBe(true);
    expect(new URL(alien.url()).host).toBe(new URL(base).host);
    expect(upstreamRequests).toBe(beforeAlien);
    const denied = await api(`/relay/devices/${deviceId}/port-mappings/${mapping.id}/open`, 'POST', { path: '/' }, other);
    expect(denied.status).toBe(403);
    rows.push({ phase: 'security', otherAccountOpenStatus: denied.status, reachedFixture: false });
    // Account session retained, preview cookie removed: measure PortPreviewPage handoff.
    const coldContext = ownerContext; await coldContext.clearCookies({ name: 'rc_port_preview' });
    const handoffPage = await coldContext.newPage(); instrument(handoffPage, 'owner-handoff');
    rows.push({ phase: 'owner-handoff', cliOpenNavigationMs: await navigate(handoffPage, mapping.openUrl) });
    const directContext = await context(); const direct = await directContext.newPage();
    rows.push({ phase: 'direct-upstream', rootMs: await navigate(direct, directBase), ...await interactions(direct) });
    const rtts = (process.env.E2E_PREVIEW_RTTS ?? '0,40,100,200').split(',').map(Number);
    for (const rtt of rtts) {
      oneWayMs = rtt / 2;
      const beforeConnections = dataConnections;
      const rootMs = await navigate(page, stableUrl);
      const interaction = await interactions(page);
      const assetsMs = await navigate(page, new URL('/assets', stableUrl).href);
      expect(await page.evaluate(() => (window as unknown as { assets: number }).assets)).toBe(12);
      const modulesMs = await navigate(page, new URL('/modules', stableUrl).href);
      expect(await page.evaluate(() => (window as unknown as { modules: number }).modules)).toBe(5);
      const row = { phase: 'authorized-data-path', relaySupervisorRttMs: rtt, rootMs, assets12Ms: assetsMs, modulesDepth5Ms: modulesMs,
        dataConnections: dataConnections - beforeConnections, ...interaction };
      rows.push(row);
      await writeFile(join(evidence, 'measurements.json'), JSON.stringify(rows, null, 2));
      console.log(JSON.stringify({ rttMs: rtt, rootMs: Math.round(rootMs), serial8Ms: Math.round(interaction.serialWall), parallel8Ms: Math.round(interaction.parallelWall),
        wsOpenMs: Math.round(interaction.wsOpenMs), wsEchoMedianMs: Math.round([...interaction.wsEchoMs].sort((a,b)=>a-b)[2]),
        sseFirstMs: Math.round(interaction.sseArrivalMs[0]), assets12Ms: Math.round(assetsMs), modulesDepth5Ms: Math.round(modulesMs), dataConnections: row.dataConnections }));
    }
    oneWayMs = 0;
    await cli('stop', String(port));
    expect((await ownerContext.request.get(stableUrl, { maxRedirects: 0 })).status()).toBe(404);
    rows.push({ phase: 'revocation', status: 404 });
  } finally {
    for (const c of contexts) await c.close().catch(() => {});
    for (const proc of processes) {
      if (proc.exitCode !== null || proc.signalCode !== null) continue;
      await new Promise<void>(done => { proc.once('exit', () => done()); proc.kill('SIGTERM'); setTimeout(() => proc.kill('SIGKILL'), 2000).unref(); });
    }
    for (const timer of pendingTimers) clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    await new Promise<void>(done => proxy.close(() => done()));
    for (const client of wsServer.clients) client.terminate();
    wsServer.close(); service.closeAllConnections(); await new Promise<void>(done => service.close(() => done()));
    await writeFile(join(evidence, 'measurements.json'), JSON.stringify(rows, null, 2));
    // Deliberately omit headers, bodies, cookies, all query strings and concrete IDs.
    await writeFile(join(evidence, 'network-timings.redacted.json'), JSON.stringify(network, null, 2));
    await writeFile(join(evidence, 'metadata.json'), JSON.stringify({ binarySha256,
      upstreamCounts, model: 'application bytes delayed each direction; loopback TCP, no TLS; no public access', cleanup: 'disposable processes terminated and data removed' }, null, 2));
    await rm(root, { recursive: true, force: true });
  }
});
