import { test, expect } from '@playwright/test';
import WebSocket from 'ws';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';
import { createServer } from 'node:net';

// Isolated real relay + fake-harness device; no production credentials or files.
test('relay enforces attachment, websocket, browser-origin and revocable-session boundaries', async ({
  browser,
}) => {
  const root = await mkdtemp(resolve('.local/security-regression-'));
  const procs: ChildProcess[] = [],
    sockets: WebSocket[] = [];
  const freePort = () =>
    new Promise<number>((done) => {
      const server = createServer();
      server.listen(0, '127.0.0.1', () => {
        const port = (server.address() as { port: number }).port;
        server.close(() => done(port));
      });
    });
  const rp = await freePort(),
    sp = await freePort();
  const base = `http://127.0.0.1:${rp}`,
    local = `http://127.0.0.1:${sp}`,
    wsBase = base.replace('http:', 'ws:');
  const password = randomBytes(24).toString('hex'),
    nextPassword = randomBytes(24).toString('hex');
  const env = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key),
    ),
  );
  function start(command: string, extra: Record<string, string>) {
    const proc = spawn(resolve('target/debug/pockymoe'), [command], {
      env: {
        ...env,
        HOST: '127.0.0.1',
        POCKYMOE_ADMIN_USERNAME: 'testadmin',
        POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_E2E_FAKE_RUNTIME: '1',
        ...extra,
      },
      stdio: 'ignore',
    });
    procs.push(proc);
  }
  async function request(
    url: string,
    method = 'GET',
    body?: unknown,
    token?: string,
    headers: Record<string, string> = {},
  ) {
    const response = await fetch(url, {
      method,
      headers: {
        ...(body ? { 'Content-Type': 'application/json' } : {}),
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
        ...headers,
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
      signal: AbortSignal.timeout(10000),
    }).catch((cause) => {
      throw new Error(`${method} ${new URL(url).pathname} failed`, { cause });
    });
    const text = await response.text();
    let data: any;
    try {
      data = JSON.parse(text);
    } catch {
      /* binary/html */
    }
    return { status: response.status, data, text, headers: response.headers };
  }
  function ok(result: Awaited<ReturnType<typeof request>>) {
    expect(result.status, result.text.slice(0, 200)).toBeGreaterThanOrEqual(
      200,
    );
    expect(result.status).toBeLessThan(300);
    return result.data;
  }
  async function login(name: string, pw = password) {
    return ok(
      await request(`${base}/relay/auth/login`, 'POST', {
        username: name,
        password: pw,
      }),
    ).token as string;
  }
  async function account(name: string) {
    ok(
      await request(`${base}/relay/auth/register`, 'POST', {
        username: name,
        email: `${name}@example.test`,
        password,
      }),
    );
    return login(name);
  }
  async function open(url: string, headers: Record<string, string>) {
    const ws = new WebSocket(url, { headers });
    sockets.push(ws);
    await new Promise<void>((done, reject) => {
      ws.once('open', done);
      ws.once('error', reject);
    });
    return ws;
  }
  try {
    start('relay', {
      PORT: String(rp),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
    });
    await expect
      .poll(async () =>
        request(`${base}/healthz`)
          .then((r) => r.status)
          .catch(() => 0),
      )
      .toBe(200);
    let owner = await account('owner');
    const guest = await account('guest');
    // This spec shares through the relay API and tests access, not the
    // safety-code check (workspace-sharing.spec.ts covers it): turn it off.
    ok(await request(`${base}/relay/account/preferences`, 'PATCH', { requireSafetyCode: false }, owner));
    const device = ok(
      await request(
        `${base}/relay/devices`,
        'POST',
        { name: 'Test device' },
        owner,
      ),
    );
    const api = `${base}/relay/devices/${device.device.id}/api`;
    start('relay-supervisor', {
      PORT: String(sp),
      POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: base,
      POCKYMOE_RELAY_AGENT_TOKEN: device.token,
      POCKYMOE_DATABASE_PATH: join(root, 'supervisor.sqlite'),
      POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces'),
    });
    await expect
      .poll(
        async () =>
          (await request(`${base}/healthz`)).data.connectedSupervisors,
      )
      .toBe(1);
    expect(
      (
        await request(`${local}/api/workspaces`, 'GET', undefined, undefined, {
          'x-pockymoe-relay-forwarded': '1',
        })
      ).status,
    ).toBe(401);
    const work = join(root, 'workspace');
    await mkdir(work);
    await writeFile(join(work, 'private.txt'), 'SYNTHETIC_PRIVATE_FILE');
    const workspace = ok(
      await request(
        `${api}/workspaces`,
        'POST',
        { label: 'Test', absPath: work },
        owner,
      ),
    );
    const thread = ok(
      await request(
        `${api}/threads/start`,
        'POST',
        { workspaceId: workspace.id, model: 'fake', title: 'Test thread' },
        owner,
      ),
    );
    const tid: string = thread.thread?.id ?? thread.id;
    ok(
      await request(
        `${base}/relay/shares`,
        'POST',
        {
          deviceId: device.device.id,
          threadId: tid,
          targetUsername: 'guest',
          threadAccess: 'read',
          workspaceAccess: 'none',
        },
        owner,
      ),
    );
    const denied = await request(
      `${api}/threads/${tid}/assets/image?path=private.txt`,
      'GET',
      undefined,
      guest,
    );
    expect(denied.status).toBe(400);
    expect(denied.text).not.toContain('SYNTHETIC_PRIVATE_FILE');
    await mkdir(join(work, '.temp/threads', tid), { recursive: true });
    await writeFile(
      join(work, '.temp/threads', tid, 'ok.png'),
      Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=',
        'base64',
      ),
    );
    expect(
      (
        await request(
          `${api}/threads/${tid}/assets/image?path=./.temp/threads/${tid}/ok.png`,
          'GET',
          undefined,
          guest,
        )
      ).status,
    ).toBe(200);
    expect(
      JSON.stringify(
        ok(await request(`${base}/relay/devices`, 'GET', undefined, owner)),
      ),
    ).not.toContain(device.token);

    const setupUrl = `${base}/relay/devices/${device.device.id}/setup-token`;
    expect((await request(setupUrl, 'POST')).status).toBe(401);
    expect((await request(setupUrl, 'POST', undefined, guest)).status).toBe(404);
    const setup = await request(setupUrl, 'POST', undefined, owner);
    expect(ok(setup).token).toBe(device.token);
    expect(setup.headers.get('cache-control')).toBe('no-store');
    expect(ok(await request(setupUrl, 'POST', undefined, owner)).token).toBe(device.token);

    // An inert second device makes attempted unauthorized forwarding observable.
    const inert = ok(
      await request(
        `${base}/relay/devices`,
        'POST',
        { name: 'Inert tunnel' },
        owner,
      ),
    );
    // Pre-0.12.16 devices have no Origin and send their device token in the URL.
    await expect(
      open(`${wsBase}/supervisor/tunnel?token=invalid-device-token`, {}),
    ).rejects.toThrow('401');
    await expect(
      open(
        `${wsBase}/supervisor/tunnel?token=${encodeURIComponent(inert.token)}`,
        { Origin: 'https://attacker.example' },
      ),
    ).rejects.toThrow('403');
    const tunnel = await open(
      `${wsBase}/supervisor/tunnel?token=${encodeURIComponent(inert.token)}`,
      {},
    );
    const frames: any[] = [];
    tunnel.on('message', (data) => frames.push(JSON.parse(data.toString())));
    ok(
      await request(
        `${base}/relay/shares`,
        'POST',
        {
          deviceId: inert.device.id,
          threadId: 'shared-thread',
          targetUsername: 'guest',
          threadAccess: 'control',
          workspaceAccess: 'none',
        },
        owner,
      ),
    );
    const shared = await open(
      `${wsBase}/relay/devices/${inert.device.id}/ws?threadId=shared-thread`,
      { Authorization: `Bearer ${guest}` },
    );
    shared.send(JSON.stringify({ type: 'supervisor.ping' }));
    await expect
      .poll(() =>
        frames.some(
          (f) =>
            f.type === 'relay.client.message' &&
            f.payload.type === 'supervisor.ping',
        ),
      )
      .toBe(true);
    const closed = new Promise<void>((done) =>
      shared.once('close', () => done()),
    );
    shared.send(
      JSON.stringify({
        type: 'shell.attach',
        threadId: 'private-thread',
        shellId: 'private-shell',
        cols: 80,
        rows: 24,
      }),
    );
    await closed;
    expect(frames.some((f) => f.payload?.shellId === 'private-shell')).toBe(
      false,
    );
    await expect(
      open(`${wsBase}/relay/devices/${inert.device.id}/ws`, {
        Origin: 'https://foreign.example',
        Cookie: `remote_codex_relay_session=${owner}`,
      }),
    ).rejects.toThrow(/403/);
    expect(
      (
        await request(
          `${base}/relay/auth/logout`,
          'POST',
          undefined,
          undefined,
          {
            Origin: 'https://foreign.example',
            Cookie: `remote_codex_relay_session=${owner}`,
          },
        )
      ).status,
    ).toBe(403);

    await writeFile(
      join(work, 'untrusted.html'),
      '<html><body><script>document.body.textContent="EXECUTED:"+localStorage.getItem("synthetic-marker")</script></body></html>',
    );
    const context = await browser.newContext();
    try {
      await context.addCookies([
        { name: 'remote_codex_relay_session', value: owner, url: base },
      ]);
      await context.addInitScript(() =>
        localStorage.setItem('synthetic-marker', 'not-a-secret'),
      );
      const page = await context.newPage();
      const response = await page.goto(
        `${api}/workspaces/${workspace.id}/files/raw?path=untrusted.html`,
      );
      expect(response?.headers()['content-security-policy']).toContain(
        'sandbox',
      );
      expect(await page.locator('body').innerText()).not.toContain('EXECUTED:');
    } finally {
      await context.close();
    }

    const live = await open(`${wsBase}/relay/devices/${inert.device.id}/ws`, {
      Authorization: `Bearer ${owner}`,
    });
    ok(await request(`${base}/relay/auth/logout`, 'POST', undefined, owner));
    expect(
      (await request(`${base}/relay/devices`, 'GET', undefined, owner)).status,
    ).toBe(401);
    await expect.poll(() => live.readyState).toBe(WebSocket.CLOSED);
    owner = await login('owner');
    ok(
      await request(
        `${base}/relay/account/password`,
        'PATCH',
        { currentPassword: password, newPassword: nextPassword },
        owner,
      ),
    );
    expect(
      (await request(`${base}/relay/devices`, 'GET', undefined, owner)).status,
    ).toBe(401);
    const fresh = await request(`${base}/relay/auth/login`, 'POST', {
      username: 'owner',
      password: nextPassword,
    });
    ok(fresh);
    expect(fresh.headers.get('set-cookie')).toContain('; Secure;');
    expect(
      (
        await request(
          `${base}/relay/devices/${device.device.id}/token`,
          'POST',
          undefined,
          guest,
        )
      ).status,
    ).toBe(404);
    const rotated = ok(
      await request(
        `${base}/relay/devices/${device.device.id}/token`,
        'POST',
        undefined,
        fresh.data.token,
      ),
    );
    expect(rotated.token).not.toBe(device.token);
    await expect(
      open(`${wsBase}/supervisor/tunnel`, {
        Authorization: `Bearer ${device.token}`,
      }),
    ).rejects.toThrow(/401/);
    const newTunnel = await open(`${wsBase}/supervisor/tunnel`, {
      Authorization: `Bearer ${rotated.token}`,
    });
    newTunnel.close();
    expect(
      JSON.stringify(
        ok(
          await request(
            `${base}/relay/devices`,
            'GET',
            undefined,
            fresh.data.token,
          ),
        ),
      ),
    ).not.toContain(rotated.token);
  } finally {
    for (const ws of sockets) ws.terminate();
    await Promise.all(
      procs.map(
        (proc) =>
          new Promise<void>((done) => {
            if (proc.exitCode !== null) return done();
            proc.once('exit', () => done());
            proc.kill('SIGTERM');
            const timer = setTimeout(() => proc.kill('SIGKILL'), 2000);
            timer.unref();
          }),
      ),
    );
    await rm(root, { recursive: true, force: true });
  }
});
