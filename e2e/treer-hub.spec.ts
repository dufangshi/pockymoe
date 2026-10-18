import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { once } from 'node:events';
import WebSocket from 'ws';

test('Treer UI prefix opens only the bound thread and accepts a prompt', async ({ page }) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), 'pockymoe-treer-web-'));
  const reserve = net.createServer(); reserve.listen(0, '127.0.0.1'); await once(reserve, 'listening');
  const port = (reserve.address() as net.AddressInfo).port;
  await new Promise<void>(resolve => reserve.close(() => resolve()));
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^(POCKYMOE|REMOTE_CODEX|TREER)_/.test(key)));
  const child = spawn(path.resolve('target/debug/pockymoe'), ['supervisor'], {
    env: { ...env, POCKYMOE_MODE: 'local', POCKYMOE_HOST_MODE: 'hosted', POCKYMOE_HOSTED_BY: 'treer',
      POCKYMOE_E2E_FAKE_RUNTIME: '1', POCKYMOE_DATABASE_PATH: path.join(dir, 'hub.sqlite'),
      POCKYMOE_WORKSPACE_ROOT: dir, POCKYMOE_ENABLED_AGENT_PROVIDERS: 'codex',
      POCKYMOE_WEB_DIST_DIR: process.env.POCKYMOE_E2E_WEB_DIST ?? path.resolve('apps/supervisor-web/dist'),
      HOST: '127.0.0.1', PORT: String(port) }, stdio: ['ignore', 'ignore', 'pipe'],
  });
  let startupLog = ''; child.stderr?.on('data', data => { startupLog = (startupLog + String(data)).slice(-4000); });
  const base = `http://127.0.0.1:${port}`;
  let attach: WebSocket | undefined;
  let proxy: http.Server | undefined;
  const sockets = new Set<net.Socket>();
  const requests: Array<{ method: string; path: string; status: number }> = [];
  try {
    await expect.poll(async () => { if (child.exitCode != null) throw new Error('Isolated fake Supervisor exited: ' + startupLog); return new Promise<number>(resolve => { const probe = http.get(base + '/healthz', response => { response.resume(); resolve(response.statusCode ?? 0); }); probe.on('error', () => resolve(0)); }); }).toBe(200);
    const token = (await readFile(path.join(dir, 'treer-attach.token'), 'utf8')).trim();
    attach = new WebSocket(`ws://127.0.0.1:${port}/api/treer/attach`, { headers: { authorization: `Bearer ${token}` } });
    await once(attach, 'open');
    const reply = once(attach, 'message');
    attach.send(JSON.stringify({ protocolVersion: 1, agentId: 'ag-browser', instanceId: 'browser-instance',
      create: { cwd: dir, provider: 'codex', model: 'ios-e2e-stream', title: 'Treer bound thread' }, treer: {} }));
    const bound = JSON.parse(String((await reply)[0]));
    expect(bound.type).toBe('bound');
    // An unrelated local thread is visible to ordinary local callers, but not to this tunnel.
    const detail = await fetch(`${base}/api/threads/${bound.threadId}`).then(r => r.json());
    const other = await fetch(base + '/api/threads/start', { method: 'POST', headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ workspaceId: detail.thread.workspaceId, model: 'ios-e2e-stream', provider: 'codex', title: 'Private other thread' }) }).then(r => r.json());
    const prefix = '/treer-ui/ag-browser/';
    const forward = (request: http.IncomingMessage) => {
      const headers = Object.fromEntries(Object.entries(request.headers).filter(([key]) => !/^x[-_]treer[-_]/i.test(key) && !['cookie', 'authorization'].includes(key)));
      return { host: '127.0.0.1', port, path: request.url!.slice(prefix.length - 1), method: request.method,
        headers: { ...headers, host: `127.0.0.1:${port}`, 'x-treer-interface-source': 'proxy-ui',
          'x-treer-agent-id': 'ag-browser', 'x-treer-interface-instance': 'browser-instance' } };
    };
    proxy = http.createServer((request, response) => {
      if (!request.url?.startsWith(prefix)) { response.writeHead(404).end(); return; }
      const options = forward(request);
      const upstream = http.request(options, incoming => {
        requests.push({ method: request.method!, path: options.path, status: incoming.statusCode! });
        response.writeHead(incoming.statusCode!, incoming.headers); incoming.pipe(response);
      });
      upstream.on('error', () => response.writeHead(502).end()); request.pipe(upstream);
    });
    proxy.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
    proxy.on('upgrade', (request, socket, head) => {
      const upstream = http.request(forward(request));
      upstream.on('upgrade', (response, peer, peerHead) => {
        socket.write(`HTTP/1.1 101 Switching Protocols\r\n${Object.entries(response.headers).map(([key, value]) => `${key}: ${value}`).join('\r\n')}\r\n\r\n`);
        if (head.length) peer.write(head); if (peerHead.length) socket.write(peerHead);
        socket.pipe(peer).pipe(socket); socket.on('close', () => peer.destroy()); peer.on('error', () => socket.destroy());
      });
      upstream.on('error', () => socket.destroy()); upstream.end();
    });
    proxy.listen(0, '127.0.0.1'); await once(proxy, 'listening');
    const proxyBase = `http://127.0.0.1:${(proxy.address() as net.AddressInfo).port}${prefix}`;
    await page.goto(proxyBase);
    await expect(page).toHaveURL(proxyBase + 'threads/' + bound.threadId);
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
    await expect(page.getByText('Private other thread', { exact: true })).toHaveCount(0);
    await page.getByRole('textbox', { name: 'Prompt' }).fill('hello, reply me with hello');
    await page.getByRole('button', { name: 'Send Prompt' }).click();
    await expect(page.getByText('hello', { exact: true })).toBeVisible();
    await expect(page.getByText('Showing 1 of 1 turns')).toBeVisible();
    const denied = await page.evaluate(async ({ prefix, otherId }) => {
      const results = [];
      for (const endpoint of [`api/threads/${otherId}`, 'api/management/supervisor', 'api/device/metrics']) results.push((await fetch(prefix + endpoint)).status);
      return results;
    }, { prefix, otherId: other.id });
    expect(denied).toEqual([403, 403, 403]);
    // Settings keep this browser's own preferences; device administration stays out.
    await page.getByRole('button', { name: 'Open settings' }).first().click();
    const settings = page.getByTestId('settings-dialog');
    await expect(settings.getByRole('tab')).toHaveText(['Session', 'Preferences']);
    await settings.getByRole('tab', { name: 'Preferences' }).click();
    await expect(settings.getByText('Model pricing')).toHaveCount(0);
    // A built-in theme pack loads through the tunnel prefix instead of falling back to Classic.
    await settings.locator('label[data-preset="plum-pocket"]').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', 'plum-pocket');
    expect(requests.filter(r => r.path.startsWith('/themes/') && r.status !== 200)).toEqual([]);
    expect(requests.some(r => r.path.startsWith('/themes/plum-pocket.css'))).toBe(true);
    await settings.locator('label[data-preset="classic"]').click();
    await expect(page.locator('html')).toHaveAttribute('data-theme-preset', 'classic');
    await page.keyboard.press('Escape');
    // Keep the observed page dependencies visible in the test evidence.
    // Top-level pages need no "open in a new tab" button.
    await expect(page.getByRole('button', { name: 'Open in a new browser tab' })).toHaveCount(0);
    // Inside a Treer frame that allows popups the button opens this page as a full tab;
    // in an older frame without them it explains Treer's own menu entry instead.
    const sandbox = 'allow-scripts allow-forms allow-same-origin allow-modals allow-downloads';
    for (const extra of [' allow-popups allow-popups-to-escape-sandbox', '']) {
      await page.setContent(`<iframe style="width:1200px;height:680px" sandbox="${sandbox}${extra}" src="${proxyBase}threads/${bound.threadId}"></iframe>`);
      const frame = page.frameLocator('iframe');
      const open = frame.getByRole('button', { name: 'Open in a new browser tab' });
      await expect(open).toBeVisible();
      if (extra) {
        const [tab] = await Promise.all([page.context().waitForEvent('page'), open.click()]);
        await expect(tab).toHaveURL(proxyBase + 'threads/' + bound.threadId);
        await expect(tab.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
        await expect(tab.getByRole('button', { name: 'Open in a new browser tab' })).toHaveCount(0);
        await tab.close();
      } else {
        await open.click();
        await expect(frame.getByRole('status').filter({ hasText: 'Open in new browser tab' })).toBeVisible();
      }
    }
    console.log('Treer UI request audit:', JSON.stringify(requests.filter(r => r.path.startsWith('/api/') || r.status >= 400)));
  } finally {
    await page.close(); attach?.terminate();
    for (const socket of sockets) socket.destroy(); proxy?.close();
    if (child.exitCode == null && child.signalCode == null) { const exited = once(child, 'exit'); child.kill('SIGTERM'); await exited; }
    await rm(dir, { recursive: true, force: true });
  }
});
