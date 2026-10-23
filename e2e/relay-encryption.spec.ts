import { test, expect, type Browser, type BrowserContext } from '@playwright/test';
import WebSocket from 'ws';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes, webcrypto } from 'node:crypto';
import { createServer } from 'node:net';

// Isolated real relay + fake-harness device; no production credentials or files.
test.use({ actionTimeout: 15000 });
test('device key failures distinguish offline and reconnecting devices without downgrading encryption', async ({ page }) => {
  await page.goto('/');
  const failures = [
    { status: 503, body: { code: 'service_unavailable', message: 'device is offline' }, code: 'device_offline', message: 'Wake it' },
    { status: 504, body: '<html>Gateway timeout</html>', code: 'device_unresponsive', message: 'may be asleep or reconnecting' },
    { status: 403, body: {}, code: 'transport_unavailable', message: 'no longer have access' },
    { status: 429, body: {}, code: 'transport_unavailable', message: 'busy' },
    { status: 503, body: {}, code: 'transport_unavailable', message: 'temporarily unavailable' },
    { status: 404, body: {}, code: 'transport_unavailable', message: 'encrypted device connection' },
  ];
  let attempts = 0;
  await page.route('**/relay/devices/sleeping-device/api/transport/key?*', route => {
    const failure = failures[attempts++]!;
    return route.fulfill({ status: failure.status, contentType: 'application/json', body: typeof failure.body === 'string' ? failure.body : JSON.stringify(failure.body) });
  });
  const results = await page.evaluate(async count => {
    // Use the same transport code as the browser and Service Worker, with real IndexedDB.
    const modulePath = '/src/lib/relayTransportCrypto.ts';
    const transport = await import(/* @vite-ignore */ modulePath);
    await transport.resetPinnedDevice('sleeping-device');
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open('remote-codex-transport-v1', 1);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('identities', 'readwrite');
      tx.objectStore('identities').put('saved-identity', 'sleeping-device');
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
    db.close();
    const results = [];
    for (let i = 0; i < count; i++) {
      const { response } = await transport.exchange(new Request(`${location.origin}/relay/devices/sleeping-device/api/threads`));
      results.push({ status: response.status, ...await response.json() });
    }
    return results;
  }, failures.length);
  expect(attempts).toBe(failures.length); // Failed key promises must not poison retries.
  for (const [index, result] of results.entries()) {
    expect(result.status).toBe(failures[index]!.status);
    expect(result.code).toBe(failures[index]!.code);
    expect(result.message).toContain(failures[index]!.message);
  }
});

test('encrypted relay interoperates with Rust for HTTP, attachments, terminal and public snapshots', async ({
  browser,
  context,
}) => {
  await encryptedRelayScenario(browser, context, false);
});

test('shared device workspace collection performs an encrypted handshake and respects revocation', async ({ browser, context }) => {
  await encryptedRelayScenario(browser, context, true);
});

test('scheduled history with encoded IDs expands over encrypted relay for owners and readers', async ({ browser, context }) => {
  await encryptedRelayScenario(browser, context, false, true);
});

async function encryptedRelayScenario(browser: Browser, context: BrowserContext, sharedOnly: boolean, historyOnly = false) {
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
    const proc = spawn(
      resolve(process.env.E2E_SECURITY_BINARY ?? 'target/debug/pockymoe'),
      [command],
      {
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
      },
    );
    procs.push(proc);
    return proc;
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
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
      POCKYMOE_PUBLIC_BASE_URL: base,
    });
    await expect
      .poll(async () =>
        request(`${base}/healthz`)
          .then((r) => r.status)
          .catch(() => 0),
      )
      .toBe(200);
    const owner = await account('owner');
    // This spec shares through the relay API and tests access, not the
    // safety-code check (workspace-sharing.spec.ts covers it): turn it off.
    ok(await request(`${base}/relay/account/preferences`, 'PATCH', { requireSafetyCode: false }, owner));
    const device = ok(
      await request(
        `${base}/relay/devices`,
        'POST',
        { name: 'Encrypted test device' },
        owner,
      ),
    );
    const api = `${base}/relay/devices/${device.device.id}/api`;
    const supervisorEnv = {
      PORT: String(sp),
      POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: base,
      POCKYMOE_RELAY_AGENT_TOKEN: device.token,
      POCKYMOE_DATABASE_PATH: join(root, 'supervisor.sqlite'),
      POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces'),
    };
    const supervisor = start('relay-supervisor', supervisorEnv);
    await expect
      .poll(
        async () =>
          (await request(`${base}/healthz`)).data.connectedSupervisors,
      )
      .toBe(1);
    const work = join(root, 'workspace');
    await mkdir(work);
    const workspace = ok(
      await request(
        `${api}/workspaces`,
        'POST',
        { label: 'Encrypted workspace', absPath: work },
        owner,
      ),
    );
    const thread = ok(
      await request(
        `${api}/threads/start`,
        'POST',
        { workspaceId: workspace.id, model: 'fake', title: 'Encrypted thread' },
        owner,
      ),
    );
    const tid: string = thread.thread?.id ?? thread.id;
    await mkdir(join(work, '.temp/threads', tid), { recursive: true });
    await writeFile(
      join(work, '.temp/threads', tid, 'ok.png'),
      Buffer.from(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aD1sAAAAASUVORK5CYII=',
        'base64',
      ),
    );
    await writeFile(join(work, 'private.txt'), 'ENCRYPTED_FILE_MARKER');
    await writeFile(
      join(work, 'chunked.bin'),
      Buffer.alloc(3 * 1024 * 1024 + 17, 0x42),
    );
    ok(
      await request(
        `${api}/threads/${tid}/prompt`,
        'POST',
        { prompt: `PUBLIC_PROMPT_MARKER [PHOTO ./.temp/threads/${tid}/ok.png ]` },
        owner,
      ),
    );
    await expect
      .poll(async () =>
        JSON.stringify(
          (await request(`${api}/threads/${tid}`, 'GET', undefined, owner))
            .data,
        ),
      )
      .toContain('agentMessage');
    if (historyOnly) {
      const scheduledId = `${tid}:scheduled:timer-history`;
      // Seed only the isolated device database, as if a finished native timer
      // had been recovered while no browser was connected.
      execFileSync('python3', ['-c', `
import sqlite3,json,sys
db,thread,turn=sys.argv[1:]
with sqlite3.connect(db) as conn:
    # The history search triggers call functions the Supervisor registers.
    conn.create_function('search_body',3,lambda text,kind,source:text or '',deterministic=True)
    conn.create_function('search_fold',1,lambda text:(text or '').lower(),deterministic=True)
    at='2030-01-01T00:00:00Z'
    conn.execute("INSERT INTO thread_turns(id,thread_id,status,model,started_at,completed_at,ordinal) VALUES(?,?,'completed','fake',?,?,99)",(turn,thread,at,at))
    for n,(kind,text) in enumerate([('userMessage','SCHEDULED_HISTORY_PROMPT'),('commandExecution','echo HISTORY_STEP_MARKER'),('reasoning','HISTORY_REASONING '+'compressible reasoning text '*1500),('agentMessage','SCHEDULED_HISTORY_REPLY')]):
        item={'id':turn+':'+str(n),'sourceTurnId':turn,'kind':kind,'text':text,'status':'completed','createdAt':at,'sequence':n}
        if kind=='commandExecution': item['detailText']='HISTORY_STEP_MARKER'
        conn.execute('INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?)',(item['id'],thread,turn,item['id'],json.dumps(item),at,at))
`, join(root, 'supervisor.sqlite'), tid, scheduledId]);
      const reader = await account('history-reader');
      ok(await request(`${base}/relay/grants`, 'POST', {
        deviceId: device.device.id, targetIdentifier: 'history-reader', scope: 'thread', threadId: tid,
        threadAccess: 'read', workspaceAccess: 'none', canCreateThreads: false,
      }, owner));
      for (const token of [owner, reader]) {
        const historyContext = await browser.newContext();
        try {
          await historyContext.addCookies([{ name: 'remote_codex_relay_session', value: token, url: base }]);
          const page = await historyContext.newPage();
          await page.goto(`${base}/devices/${device.device.id}/threads/${tid}`);
          await expect(page.getByText('SCHEDULED_HISTORY_REPLY', { exact: true })).toBeVisible();
          const detailResponse = page.waitForResponse(r => r.url().includes(`/turns/${encodeURIComponent(scheduledId)}/detail`));
          await page.getByRole('button', { name: /Expand turn 2$/ }).click();
          const response = await detailResponse;
          expect(response.status()).toBe(200);
          expect(response.headers()['x-rcd-encrypted']).toBeTruthy();
          // ~42 KB of JSON travels gzipped inside the sealed envelope.
          expect((await response.body()).length).toBeLessThan(10_000);
          await expect(page.getByRole('button', { name: /Collapse turn 2$/ })).toBeVisible();
          await expect(page.locator('.thread-execution-timeline .error-mark')).toHaveCount(0);
          await expect(page.locator('.thread-execution-timeline')).toContainText('HISTORY_STEP_MARKER');
        } finally { await historyContext.close(); }
      }
      return;
    }
    ok(await request(`${api}/threads/${tid}/shell`, 'POST', {}, owner));
    if (sharedOnly) {
    // A shared collection page has no thread/workspace ID for its first HPKE
    // handshake. Exercise it in a fresh browser with no owner's cached key.
    const reader = await account('device-reader');
    const grant = ok(await request(`${base}/relay/grants`, 'POST', {
      deviceId: device.device.id, targetIdentifier: 'device-reader', scope: 'device',
      threadAccess: 'read', workspaceAccess: 'read', canCreateThreads: false,
    }, owner));
    const sharedContext = await browser.newContext();
    try {
      await sharedContext.addCookies([{ name: 'remote_codex_relay_session', value: reader, url: base }]);
      const sharedPage = await sharedContext.newPage();
      const keyResponse = sharedPage.waitForResponse(r => r.url().includes('/api/transport/key?'));
      const workspaceResponse = sharedPage.waitForResponse(r => r.url() === `${api}/workspaces` && !!r.headers()['x-rcd-encrypted']);
      await sharedPage.goto(`${base}/devices/${device.device.id}/workspaces`);
      expect((await keyResponse).status()).toBe(200);
      expect((await workspaceResponse).status()).toBe(200);
      await expect(sharedPage.getByText('Encrypted workspace', { exact: true })).toBeVisible();
      await expect(sharedPage.getByText('You no longer have access to this device.', { exact: true })).toHaveCount(0);
      expect((await request(`${api}/management/upstreams`, 'GET', undefined, reader)).status).toBe(403);
      ok(await request(`${base}/relay/grants/${grant.id}`, 'DELETE', undefined, owner));
      expect((await request(`${api}/transport/key?challenge=revoked`, 'GET', undefined, reader)).status).toBe(401);
      expect((await request(`${api}/workspaces`, 'GET', undefined, reader)).status).toBe(401);
    } finally { await sharedContext.close(); }
    return;
    }
    await context.addCookies([
      { name: 'remote_codex_relay_session', value: owner, url: base },
    ]);
    const page = await context.newPage();
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text());
    });
    const frames: string[] = [];
    page.on('websocket', (ws) => {
      ws.on('framesent', (f) => frames.push(String(f.payload)));
      ws.on('framereceived', (f) => frames.push(String(f.payload)));
    });
    const encrypted: string[] = [];
    let replayRequest:
      | { url: string; headers: Record<string, string> }
      | undefined;
    page.on('request', (r) => {
      if (r.headers()['x-rcd-key'] && r.method() === 'GET')
        replayRequest = { url: r.url(), headers: r.headers() };
    });
    context.on('response', async (r) => {
      if (r.headers()['x-rcd-encrypted']) encrypted.push(r.url());
    });
    await page.goto(`${base}/devices/${device.device.id}/threads/${tid}`);
    await expect
      .poll(() => encrypted.length, {
        message:
          'Encrypted HTTP responses; browser errors: ' + errors.join(' | '),
      })
      .toBeGreaterThan(0);
    await expect(
      page.getByRole('button', { name: 'Switch to shell', exact: true }),
    ).toBeVisible();
    await page
      .getByRole('button', { name: 'Switch to shell', exact: true })
      .click();
    await expect(page.locator('.shell-pane-active .xterm')).toBeVisible();
    if (test.info().project.name.includes('mobile'))
      await page.locator('.shell-pane-active .xterm-viewport').tap();
    else await page.locator('.shell-pane-active .xterm-screen').click();
    const input = page.locator('.shell-pane-active .xterm-helper-textarea');
    await input.pressSequentially("printf 'CIPHER_PTY_MARKER\n'");
    await input.press('Enter');
    await expect(page.locator('.shell-pane-active')).toContainText(
      'CIPHER_PTY_MARKER',
    );
    expect(frames.some((f) => f.includes('CIPHER_PTY_MARKER'))).toBe(false);
    expect(frames.some((f) => f.includes('"encrypted"'))).toBe(true);
    const asset = `${api}/threads/${tid}/assets/image?path=./.temp/threads/${tid}/ok.png`;
    const width = await page.evaluate(async (src) => {
      const image = new Image();
      image.src = src;
      document.body.append(image);
      await image.decode();
      return image.naturalWidth;
    }, asset);
    expect(width).toBe(1);
    const file = await page.evaluate(
      async (url) => (await fetch(url)).text(),
      `${api}/workspaces/${workspace.id}/files/raw?path=private.txt`,
    );
    expect(file).toBe('ENCRYPTED_FILE_MARKER');
    const large = await page.evaluate(async (url) => {
      const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
      return { length: bytes.length, valid: bytes.every((v) => v === 0x42) };
    }, `${api}/workspaces/${workspace.id}/files/download?path=chunked.bin`);
    expect(large).toEqual({ length: 3 * 1024 * 1024 + 17, valid: true });
    // Playwright exposes service-worker-owned network events only in Chromium.
    if (browser.browserType().name() === 'chromium')
      expect(encrypted.some((url) => url.includes('/transport/stream/'))).toBe(
        true,
      );
    expect(errors.filter((e) => !e.includes('favicon'))).toEqual([]);
    expect(replayRequest).toBeDefined();
    const replay = await fetch(replayRequest!.url, {
      headers: { ...replayRequest!.headers, authorization: `Bearer ${owner}` },
    });
    expect(replay.status).toBe(400);
    // New public links require an explicit client-projected snapshot, not relay-side reads.
    expect(
      (
        await request(
          `${base}/relay/thread-links`,
          'POST',
          { deviceId: device.device.id, threadId: tid },
          owner,
        )
      ).status,
    ).toBe(400);
    await page
      .getByRole('button', { name: 'Thread actions', exact: true })
      .click();
    await page
      .getByRole('button', { name: 'Share as link', exact: true })
      .click();
    await page
      .getByRole('button', { name: 'Create share link', exact: true })
      .click();
    await expect(
      page.getByRole('textbox', { name: 'Public share URL' }),
    ).toBeVisible();
    const publicUrl = await page
      .getByRole('textbox', { name: 'Public share URL' })
      .inputValue();
    const anonymous = await browser.newContext();
    const publicPage = await anonymous.newPage();
    await publicPage.goto(publicUrl);
    await expect(publicPage.locator('body')).toContainText(
      'PUBLIC_PROMPT_MARKER',
    );
    await expect(publicPage.locator('img')).toHaveCount(1);
    await expect.poll(() => publicPage.locator('img').evaluate((image: HTMLImageElement) => image.naturalWidth)).toBe(1);
    await expect(publicPage.locator('img')).toHaveAttribute('src', /^data:image\/png;base64,/);
    await expect(
      publicPage.getByRole('button', { name: 'Thread actions', exact: true }),
    ).toHaveCount(0);
    await anonymous.close();
    // A separate transport client primes its key once and stays idle through
    // restart. Its FIRST request afterwards is POST session, without a GET or
    // another socket reconnect accidentally refreshing the key first.
    const recoveryPage = await context.newPage();
    await recoveryPage.goto('/');
    const sessionStatuses: number[] = [];
    await recoveryPage.route(`**/relay/devices/${device.device.id}/**`, async route => {
      const url = new URL(route.request().url());
      const response = await route.fetch({
        url: `${base}${url.pathname}${url.search}`,
        headers: { ...route.request().headers(), authorization: `Bearer ${owner}`, origin: base, referer: `${base}/` },
      });
      if (url.pathname.endsWith('/transport/session')) sessionStatuses.push(response.status());
      await route.fulfill({ response });
    });
    const recoveryApi = `/relay/devices/${device.device.id}/api`;
    expect(await recoveryPage.evaluate(async api => {
      const modulePath = '/src/lib/relayTransportCrypto.ts';
      const { exchange } = await import(/* @vite-ignore */ modulePath);
      return (await exchange(new Request(`${location.origin}${api}/threads`))).response.status;
    }, recoveryApi)).toBe(200);
    // Restart only this fixture's device: persistent identity stays, ephemeral keys change.
    // Keep the page and service worker alive so both must recover their stale key cache.
    await new Promise<void>((done) => {
      supervisor.once('exit', () => done());
      supervisor.kill('SIGTERM');
    });
    await expect
      .poll(
        async () =>
          (await request(`${base}/healthz`)).data.connectedSupervisors,
      )
      .toBe(0);
    start('relay-supervisor', supervisorEnv);
    await expect
      .poll(
        async () =>
          (await request(`${base}/healthz`)).data.connectedSupervisors,
      )
      .toBe(1);
    const recoveredSession = await recoveryPage.evaluate(async api => {
      const modulePath = '/src/lib/relayTransportCrypto.ts';
      const { exchange } = await import(/* @vite-ignore */ modulePath);
      const result = await exchange(new Request(`${location.origin}${api}/transport/session`, {
        method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}',
      }), undefined, undefined, true);
      return { status: result.response.status, body: await result.response.json(), keys: Boolean(result.sendKey && result.receiveKey) };
    }, recoveryApi);
    expect(sessionStatuses).toEqual([409, 200]);
    expect(recoveredSession.status).toBe(200);
    expect(recoveredSession.body.channelId).toBeTruthy();
    expect(recoveredSession.keys).toBe(true);
    await recoveryPage.close();
    const afterRestart = await page.evaluate(
      async (url) => (await fetch(url)).text(),
      `${api}/workspaces/${workspace.id}/files/raw?path=private.txt`,
    );
    expect(afterRestart).toBe('ENCRYPTED_FILE_MARKER');
    await expect(
      page.getByRole('button', {
        name: /Device identity changed/,
      }),
    ).toHaveCount(0);
    // A valid signature from a different device identity must never replace the saved key silently.
    const pair = await webcrypto.subtle.generateKey(
      { name: 'ECDSA', namedCurve: 'P-256' },
      true,
      ['sign', 'verify'],
    );
    const identityKey = Buffer.from(
      await webcrypto.subtle.exportKey('raw', pair.publicKey),
    ).toString('base64url');
    await page.evaluate(
      async ({ deviceId, identityKey }) => {
        const db = await new Promise<IDBDatabase>((done, fail) => {
          const r = indexedDB.open('remote-codex-transport-v1', 1);
          r.onsuccess = () => done(r.result);
          r.onerror = () => fail(r.error);
        });
        await new Promise<void>((done, fail) => {
          const tx = db.transaction('identities', 'readwrite');
          tx.objectStore('identities').put(identityKey, deviceId);
          tx.oncomplete = () => done();
          tx.onerror = () => fail(tx.error);
        });
        db.close();
      },
      { deviceId: device.device.id, identityKey },
    );
    await page.reload();
    await expect(
      page.getByRole('button', {
        name: /Device identity changed/,
      }),
    ).toBeVisible();
    await context.close();
  } finally {
    for (const ws of sockets) ws.terminate();
    await Promise.all(
      procs.map(
        (proc) =>
          new Promise<void>((done) => {
            if (proc.exitCode !== null || proc.signalCode !== null)
              return done();
            proc.once('exit', () => done());
            proc.kill('SIGTERM');
            const timer = setTimeout(() => proc.kill('SIGKILL'), 2000);
            timer.unref();
          }),
      ),
    );
    await rm(root, { recursive: true, force: true });
  }
}
