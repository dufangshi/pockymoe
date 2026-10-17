import { test, expect } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, mkdir } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createServer } from 'node:net';
import { randomBytes } from 'node:crypto';
import { DatabaseSync } from 'node:sqlite';

test('shared Claude families keep provider models, effort controls and consistent child groups without unrelated access', async ({
  browser,
}, testInfo) => {
  const root = await mkdtemp(resolve('.local/shared-group-'));
  const procs: ChildProcess[] = [];
  const free = () =>
    new Promise<number>((done) => {
      const s = createServer();
      s.listen(0, '127.0.0.1', () => {
        const port = (s.address() as { port: number }).port;
        s.close(() => done(port));
      });
    });
  const rp = await free(),
    sp = await free(),
    base = `http://127.0.0.1:${rp}`,
    password = randomBytes(24).toString('hex');
  const clean = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key),
    ),
  );
  const start = (command: string, extra: Record<string, string>) => {
    const proc = spawn(resolve('target/debug/pockymoe'), [command], {
      env: {
        ...clean,
        HOST: '127.0.0.1',
        POCKYMOE_ADMIN_USERNAME: 'admin',
        POCKYMOE_ADMIN_PASSWORD: password,
        POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_E2E_FAKE_RUNTIME: '1',
        ...extra,
      },
      stdio: 'ignore',
    });
    procs.push(proc);
  };
  const api = async (
    path: string,
    token?: string,
    body?: unknown,
    method?: string,
  ) => {
    const r = await fetch(base + path, {
      method: method ?? (body ? 'POST' : 'GET'),
      headers: {
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        'content-type': 'application/json',
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
    });
    expect(r.ok, `${path}: ${r.status} ${await r.clone().text()}`).toBeTruthy();
    return r.status === 204 ? {} : r.json();
  };
  const context = await browser.newContext();
  try {
    start('relay', {
      PORT: String(rp),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
      POCKYMOE_PUBLIC_BASE_URL: base,
    });
    await expect
      .poll(() =>
        fetch(base + '/healthz')
          .then((r) => r.status)
          .catch(() => 0),
      )
      .toBe(200);
    for (const username of ['owner', 'viewer'])
      await api('/relay/auth/register', undefined, {
        username,
        email: username + '@example.test',
        password,
      });
    const owner = (
        await api('/relay/auth/login', undefined, {
          username: 'owner',
          password,
        })
      ).token,
      viewer = (
        await api('/relay/auth/login', undefined, {
          username: 'viewer',
          password,
        })
      ).token;
    const device = await api('/relay/devices', owner, { name: 'Group device' }),
      d = device.device.id,
      deviceApi = `/relay/devices/${d}/api`;
    start('relay-supervisor', {
      PORT: String(sp),
      POCKYMOE_RELAY_SUPERVISOR_PORT: String(sp),
      POCKYMOE_RELAY_SERVER_URL: base,
      POCKYMOE_RELAY_AGENT_TOKEN: device.token,
      POCKYMOE_DATABASE_PATH: join(root, 'supervisor.sqlite'),
      POCKYMOE_WORKSPACE_ROOT: join(root, 'workspaces'),
    });
    await expect
      .poll(async () => (await api('/healthz')).connectedSupervisors)
      .toBe(1);
    const absPath = join(root, 'workspace');
    await mkdir(absPath);
    const ws = await api(deviceApi + '/workspaces', owner, {
      absPath,
      label: 'Family',
    });
    const create = async (title: string, parentThreadId?: string) => {
      const r = await api(deviceApi + '/threads/start', owner, {
        workspaceId: ws.id,
        title,
        provider: 'claude',
        model: 'claude-sonnet-5-5',
        reasoningEffort: 'high',
        approvalMode: 'yolo',
        parentThreadId,
      });
      return r.id ?? r.thread.id;
    };
    const parent = await create('Claude coordinator'),
      child = await create('Child one', parent),
      other = await create('Unrelated private thread');
    const share = await api('/relay/shares', owner, {
      targetIdentifier: 'viewer',
      deviceId: d,
      threadId: parent,
      workspaceId: ws.id,
      threadAccess: 'control',
      workspaceAccess: 'none',
    });
    const later = await create('Child two', parent);
    const secondaryPath = join(root, 'secondary-workspace');
    await mkdir(secondaryPath);
    const secondary = await api(deviceApi + '/workspaces', owner, {
      absPath: secondaryPath,
      label: 'Separate child workspace',
    });
    const cross = await api(deviceApi + '/threads/start', owner, {
      workspaceId: secondary.id,
      title: 'Cross-workspace child',
      provider: 'claude',
      model: 'claude-sonnet-5-5',
      reasoningEffort: 'high',
      approvalMode: 'yolo',
      parentThreadId: parent,
    });
    const crossId = cross.id ?? cross.thread.id;
    await expect
      .poll(async () => {
        const r = await fetch(base + deviceApi + `/threads/${later}`, {
          headers: { authorization: `Bearer ${viewer}` },
        });
        return r.status;
      })
      .toBe(200);
    expect(
      (await api(deviceApi + `/threads/${child}/models`, viewer)).some(
        (m: any) => m.model === 'claude-sonnet-5-5',
      ),
    ).toBe(true);
    const group = await api(deviceApi + `/threads/${parent}/group`, viewer);
    expect(group.map((t: any) => t.id).sort()).toEqual(
      [parent, child, later, crossId].sort(),
    );
    const database = new DatabaseSync(join(root, 'supervisor.sqlite'));
    const createdAt = new Date().toISOString();
    database
      .prepare(
        "INSERT INTO thread_turns(id,thread_id,status,ordinal,started_at,completed_at) VALUES ('watch-create',?,'completed',1,?,?)",
      )
      .run(parent, createdAt, createdAt);
    const watch = {
      id: 'watch-tool',
      kind: 'toolCall',
      text: 'CronCreate',
      status: 'completed',
      createdAt,
      detailText:
        'Tool: CronCreate\n\nInput:\n' +
        JSON.stringify({
          cron: '13,43 * * * *',
          prompt: 'Check all agent threads for results.',
          recurring: true,
        }) +
        '\n\nResult:\nScheduled recurring job watch-ui (13,43 * * * *). Auto-expires after 7 days.',
    };
    database
      .prepare(
        "INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,'watch-create',?,?,?,?)",
      )
      .run(
        'watch-tool',
        parent,
        'watch-tool',
        JSON.stringify(watch),
        createdAt,
        createdAt,
      );
    database.close();
    expect(
      (await api(deviceApi + `/threads/${parent}/watches`, viewer)).watches[0],
    ).toMatchObject({
      schedule: 'Every 30 minutes (at :13, :43)',
      prompt: 'Check all agent threads for results.',
      status: 'unconfirmed',
    });
    const denied = await fetch(base + deviceApi + `/threads/${other}`, {
      headers: { authorization: `Bearer ${viewer}` },
    });
    expect([401, 403]).toContain(denied.status);
    await context.addCookies([
      { name: 'remote_codex_relay_session', value: viewer, url: base },
    ]);
    const page = await context.newPage();
    await page.goto(`${base}/devices/${d}/threads/${parent}`);
    await page
      .getByRole('button', { name: 'Automation', exact: true })
      .click();
    const watches = page.getByRole('dialog', { name: 'Automation', exact: true });
    await expect(watches).toContainText('Every 30 minutes');
    await watches.getByRole('button', { name: 'Show details', exact: true }).click();
    await expect(watches).toContainText('Check all agent threads for results.');
    await expect(watches).toContainText('Status unconfirmed');
    await page.screenshot({ path: testInfo.outputPath('shared-watch.png') });
    await watches.getByRole('button', { name: 'Close', exact: true }).click();
    const cancelAt = new Date(Date.now() + 1).toISOString();
    const cancelled = {
      id: 'watch-cancel',
      kind: 'toolCall',
      text: 'CronDelete',
      status: 'completed',
      createdAt: cancelAt,
      detailText:
        'Tool: CronDelete\n\nInput:\n' +
        JSON.stringify({ id: 'watch-ui' }) +
        '\n\nResult:\nDeleted job watch-ui.',
    };
    const cancelling = new DatabaseSync(join(root, 'supervisor.sqlite'));
    cancelling
      .prepare(
        "INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,'watch-create',?,?,?,?)",
      )
      .run(
        'watch-cancel',
        parent,
        'watch-cancel',
        JSON.stringify(cancelled),
        cancelAt,
        cancelAt,
      );
    cancelling.close();
    expect(
      (await api(deviceApi + `/threads/${parent}/watches`, viewer)).watches,
    ).toMatchObject([{ id: 'watch-ui', status: 'deleted' }]);
    const settings = page.getByRole('button', { name: /Model and effort:/ });
    await expect(settings).toBeEnabled();
    await settings.click();
    await page.getByRole('button', { name: /^Model Sonnet/ }).click();
    await expect(page.getByRole('button', { name: /Opus 5.5/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /GPT/ })).toHaveCount(0);
    await page.getByRole('button', { name: /^Effort / }).click();
    await expect(page.getByRole('button', { name: /^high$/i })).toBeEnabled();
    await page.getByRole('button', { name: /^medium$/i }).click();
    await expect
      .poll(
        async () =>
          (await api(deviceApi + `/threads/${parent}`, viewer)).thread
            .reasoningEffort,
      )
      .toBe('medium');
    const tabs = page.getByRole('navigation', { name: 'Workspace threads' });
    await expect(
      tabs.getByRole('button', { name: 'Claude coordinator: 3 agent threads' }),
    ).toBeVisible();
    const recent = page.getByTestId('workspace-threads');
    await expect(recent.locator('summary')).toContainText('3 agent threads');
    await tabs
      .getByRole('button', { name: 'Claude coordinator: 3 agent threads' })
      .click();
    await page
      .getByRole('region', { name: 'Claude coordinator agent threads' })
      .getByRole('link', { name: 'Cross-workspace child' })
      .click();
    await expect(page).toHaveURL(new RegExp(`/threads/${crossId}$`));
    await expect(
      tabs.getByRole('button', { name: 'Claude coordinator: 3 agent threads' }),
    ).toBeVisible();
    await expect(settings).toBeEnabled();
    await page.screenshot({
      path: testInfo.outputPath('shared-claude-group.png'),
    });
    await api(`/relay/shares/${share.id}`, owner, undefined, 'DELETE');
    expect([401, 403]).toContain(
      (
        await fetch(base + deviceApi + `/threads/${later}`, {
          headers: { authorization: `Bearer ${viewer}` },
        })
      ).status,
    );
    const grant = await api('/relay/grants', owner, {
      targetIdentifier: 'viewer',
      deviceId: d,
      scope: 'thread',
      threadId: parent,
      threadAccess: 'read',
      workspaceAccess: 'none',
    });
    expect((await api(deviceApi + `/threads/${child}`, viewer)).thread.id).toBe(
      child,
    );
    expect(
      (
        await fetch(base + deviceApi + `/threads/${child}/settings`, {
          method: 'PATCH',
          headers: {
            authorization: `Bearer ${viewer}`,
            'content-type': 'application/json',
          },
          body: JSON.stringify({ reasoningEffort: 'low' }),
        })
      ).status,
    ).toBe(403);
    await api(`/relay/grants/${grant.id}`, owner, undefined, 'DELETE');
    expect([401, 403]).toContain(
      (
        await fetch(base + deviceApi + `/threads/${child}`, {
          headers: { authorization: `Bearer ${viewer}` },
        })
      ).status,
    );
  } finally {
    await context.close();
    for (const p of procs) {
      p.kill('SIGTERM');
      await Promise.race([
        new Promise((r) => p.once('exit', r)),
        new Promise((r) => setTimeout(r, 2000)),
      ]);
      if (p.exitCode === null) p.kill('SIGKILL');
    }
  }
});
