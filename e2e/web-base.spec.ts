import { expect, test } from '@playwright/test';
import { createServer, request as httpRequest } from 'node:http';
import { connect } from 'node:net';
import type { AddressInfo } from 'node:net';
import type { Duplex } from 'node:stream';
import { resolve } from 'node:path';
import { randomUUID } from 'node:crypto';
import { api, ensureWorkspaceDir } from './helpers';

test('built UI keeps assets, HTTP, WebSocket and deep-link navigation under a stripped prefix', async ({ page }) => {
  const port = Number(process.env.E2E_API_PORT ?? 8787);
  const upstream = `http://127.0.0.1:${port}`;
  // Deliberately contains /api and /workspaces: never guess the mount from
  // familiar route names in the browser URL.
  const prefix = '/api/workspaces/W/agents/A/interface/ui/';
  const requests: string[] = [];
  const upgrades: string[] = [];
  const sockets = new Set<Duplex>();
  const proxy = createServer((incoming, outgoing) => {
    requests.push(incoming.url!);
    if (!incoming.url?.startsWith(prefix)) {
      outgoing.writeHead(404).end('Escaped the UI mount');
      return;
    }
    const forward = httpRequest(`${upstream}/${incoming.url.slice(prefix.length)}`, {
      method: incoming.method,
      headers: { ...incoming.headers, host: `127.0.0.1:${port}` },
    }, (response) => {
      outgoing.writeHead(response.statusCode!, response.headers);
      response.pipe(outgoing);
    });
    forward.on('error', () => outgoing.destroy());
    incoming.pipe(forward);
  });
  proxy.on('upgrade', (incoming, socket, head) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    upgrades.push(incoming.url!);
    if (!incoming.url?.startsWith(prefix)) { socket.destroy(); return; }
    const target = connect(port, '127.0.0.1', () => {
      const headers = { ...incoming.headers, host: `127.0.0.1:${port}` };
      target.write(`GET /${incoming.url!.slice(prefix.length)} HTTP/1.1\r\n${Object.entries(headers).map(([name, value]) => `${name}: ${value}`).join('\r\n')}\r\n\r\n`);
      if (head.length) target.write(head);
      socket.pipe(target).pipe(socket);
    });
    target.on('error', () => socket.destroy());
    target.on('close', () => socket.destroy());
    socket.on('error', () => target.destroy());
    socket.on('close', () => target.destroy());
  });
  await new Promise<void>((done) => proxy.listen(0, '127.0.0.1', done));
  const origin = `http://127.0.0.1:${(proxy.address() as AddressInfo).port}`;
  const workspacePath = await ensureWorkspaceDir(resolve(process.env.E2E_WORKSPACE_ROOT ?? '.local/e2e-playwright'), `web-base-${randomUUID()}`);
  const workspace = await api<{ id: string }>(upstream, '/api/workspaces', {
    method: 'POST', body: JSON.stringify({ absPath: workspacePath, label: 'Web base fixture' }),
  });
  const created = await api<{ id: string }>(upstream, '/api/threads/start', {
    method: 'POST', body: JSON.stringify({ workspaceId: workspace.id, title: 'Prefixed thread', provider: 'acp', agentId: 'grok', model: 'ios-e2e-stream', approvalMode: 'yolo' }),
  });
  const id = created.id;
  const failures: string[] = [];
  page.on('pageerror', error => failures.push(error.message));
  page.on('response', response => { if (response.status() >= 400) failures.push(`${response.status()} ${response.url()}`); });
  try {
    const connected = page.waitForEvent('websocket', { timeout: 10_000 }).then(socket => {
      expect(socket.url()).toBe(`${origin.replace('http:', 'ws:')}${prefix}ws`);
      return socket.waitForEvent('framereceived', { timeout: 10_000 });
    });
    // Keep the failure observable even if an earlier UI assertion aborts.
    void connected.catch(() => {});
    await page.goto(`${origin}${prefix}threads/${id}`);
    await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
    await connected;
    await page.getByRole('button', { name: 'Later', exact: true }).click();
    expect(await page.evaluate(() => document.baseURI)).toBe(`${origin}${prefix}`);
    await expect.poll(() => requests.some(path => path.startsWith(`${prefix}api/threads/${id}`))).toBe(true);
    expect(requests.some(path => path.startsWith(`${prefix}assets/`) && path.includes('.js'))).toBe(true);
    expect(upgrades).toContain(`${prefix}ws`);
    // Shared components use plain anchors, then the workspace toolbar uses
    // React Router. Exercise both forms, and refresh a prefixed deep route.
    await page.locator(`a[href="${prefix}workspaces"]`).first().click();
    await expect(page).toHaveURL(`${origin}${prefix}workspaces`);
    await page.reload();
    expect(await page.evaluate(() => document.baseURI)).toBe(`${origin}${prefix}`);
    await expect(page.getByText('Web base fixture', { exact: true }).first()).toBeVisible();
    await page.getByRole('link', { name: 'Add workspace', exact: true }).click();
    await expect(page).toHaveURL(`${origin}${prefix}workspaces/new`);
    await page.reload();
    await expect(page.getByRole('button', { name: 'Existing path', exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.baseURI)).toBe(`${origin}${prefix}`);
    expect(requests.every(path => path.startsWith(prefix)), requests.join('\n')).toBe(true);
    expect(failures).toEqual([]);
  } finally {
    for (const socket of sockets) socket.destroy();
    await page.close();
    proxy.closeAllConnections();
    await new Promise<void>((done) => proxy.close(() => done()));
    await api(upstream, `/api/threads/${id}`, { method: 'DELETE' });
    await api(upstream, `/api/workspaces/${workspace.id}`, { method: 'DELETE' });
  }
});
