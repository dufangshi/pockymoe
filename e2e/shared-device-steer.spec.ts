import { expect, test } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { mkdtemp, mkdir, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { join, resolve } from 'node:path';

test('shared device control can steer a running Codex turn', async () => {
  const root = await mkdtemp(resolve('.local/shared-device-steer-'));
  const processes: ChildProcess[] = [];
  const freePort = () =>
    new Promise<number>((done) => {
      const server = createServer();
      server.listen(0, '127.0.0.1', () => {
        const port = (server.address() as { port: number }).port;
        server.close(() => done(port));
      });
    });
  const relayPort = await freePort();
  const supervisorPort = await freePort();
  const base = `http://127.0.0.1:${relayPort}`;
  const password = randomBytes(24).toString('hex');
  const cleanEnv = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key),
    ),
  );
  const start = (command: string, extra: Record<string, string>) => {
    const process = spawn(resolve('target/debug/pockymoe'), [command], {
      env: {
        ...cleanEnv,
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
    processes.push(process);
  };
  const api = async (
    path: string,
    token?: string,
    body?: unknown,
    method?: string,
  ) => {
    const response = await fetch(base + path, {
      method: method ?? (body === undefined ? 'GET' : 'POST'),
      headers: {
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        'content-type': 'application/json',
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    });
    expect(
      response.ok,
      `${path}: ${response.status} ${await response.clone().text()}`,
    ).toBeTruthy();
    return response.status === 204 ? {} : response.json();
  };

  try {
    start('relay', {
      PORT: String(relayPort),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_PUBLIC_BASE_URL: base,
    });
    await expect
      .poll(() =>
        fetch(base + '/healthz')
          .then((response) => response.status)
          .catch(() => 0),
      )
      .toBe(200);
    for (const username of ['owner', 'controller']) {
      await api('/relay/auth/register', undefined, {
        username,
        email: `${username}@example.test`,
        password,
      });
    }
    const owner = (
      await api('/relay/auth/login', undefined, {
        username: 'owner',
        password,
      })
    ).token;
    const controller = (
      await api('/relay/auth/login', undefined, {
        username: 'controller',
        password,
      })
    ).token;
    const device = await api('/relay/devices', owner, {
      name: 'Shared steer device',
    });
    const deviceId = device.device.id;
    const deviceApi = `/relay/devices/${deviceId}/api`;
    start('relay-supervisor', {
      PORT: String(supervisorPort),
      POCKYMOE_RELAY_SUPERVISOR_PORT: String(supervisorPort),
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
    const workspace = await api(deviceApi + '/workspaces', owner, {
      absPath,
      label: 'Shared steer workspace',
    });
    const created = await api(deviceApi + '/threads/start', owner, {
      workspaceId: workspace.id,
      title: 'Shared Codex thread',
      provider: 'acp',
      agentId: 'codex',
      model: 'ios-e2e-stream',
      approvalMode: 'yolo',
    });
    const threadId = created.id ?? created.thread.id;
    await api('/relay/grants', owner, {
      targetIdentifier: 'controller',
      deviceId,
      scope: 'device',
      threadAccess: 'control',
      workspaceAccess: 'none',
      canCreateThreads: false,
    });

    await api(deviceApi + `/threads/${threadId}/prompt`, controller, {
      prompt: 'Inspect this repository in depth',
      clientRequestId: 'shared-running-turn',
    });
    await expect
      .poll(
        async () =>
          (
            await api(
              deviceApi + `/threads/${threadId}?view=delivery`,
              controller,
            )
          ).thread.status,
      )
      .toBe('running');
    const running = await api(
      deviceApi + `/threads/${threadId}?view=delivery`,
      controller,
    );
    const runningTurnId = running.thread.activeTurnId;
    await api(deviceApi + `/threads/${threadId}/prompt`, controller, {
      prompt: 'Shared controller correction',
      clientRequestId: 'shared-steer',
    });
    const queued = await api(
      deviceApi + `/threads/${threadId}?view=delivery`,
      controller,
    );
    const pendingSteer = queued.pendingSteers.find(
      (item: any) => item.clientRequestId === 'shared-steer',
    );
    expect(pendingSteer).toBeTruthy();
    await api(
      deviceApi +
        `/threads/${threadId}/pending-steers/${pendingSteer.id}/steer`,
      controller,
      undefined,
      'POST',
    );
    await expect
      .poll(async () => {
        const current = await api(
          deviceApi + `/threads/${threadId}`,
          controller,
        );
        return current.turns
          .find((turn: any) => turn.id === runningTurnId)
          ?.items.some(
            (item: any) =>
              item.kind === 'userMessage' &&
              item.text === 'Shared controller correction',
          );
      })
      .toBe(true);
    const steered = await api(deviceApi + `/threads/${threadId}`, controller);
    expect(steered.thread.activeTurnId).toBe(runningTurnId);
    expect(
      steered.pendingSteers.find(
        (item: any) => item.clientRequestId === 'shared-steer',
      ),
    ).toMatchObject({ delivery: 'steer', turnId: runningTurnId });
    await api(deviceApi + `/threads/${threadId}/interrupt`, controller, {});
  } finally {
    for (const process of processes) {
      process.kill('SIGTERM');
      await Promise.race([
        new Promise((done) => process.once('exit', done)),
        new Promise((done) => setTimeout(done, 2_000)),
      ]);
      if (process.exitCode === null) process.kill('SIGKILL');
    }
    await rm(root, { recursive: true, force: true });
  }
});
