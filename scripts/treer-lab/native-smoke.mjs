import { spawn } from 'node:child_process';
import { mkdir, writeFile, readFile, cp, rm } from 'node:fs/promises';
import { randomBytes, randomUUID } from 'node:crypto';
import assert from 'node:assert/strict';
import { root, stateDir, api, until, cleanEnv, save, proxyPort, webPort } from './common.mjs';
const pg = `${stateDir}/native-postgres`;
const bins = `${stateDir}/treer-target/debug`;
const workspace = `${stateDir}/native-workspace`;
const jobs = `${workspace}/lab-jobs`;
const children = [];
const checks = [];
function start(command, args, env = {}, cwd = root) {
  const child = spawn(command, args, { cwd, env: { ...cleanEnv(), ...env }, detached: true, stdio: 'ignore' });
  children.push(child);
  child.on('error', () => {});
  return child;
}
async function pass(name, fn) { await fn(); checks.push({ name, result: 'passed' }); console.log(`PASS ${name}`); }
try {
  // Refuse to overlap the Docker lab or another service; never kill a port owner.
  const net = await import('node:net');
  for (const port of [15432, proxyPort, 18790, 18880]) {
    await new Promise((resolve, reject) => {
      const socket = net.createServer();
      socket.once('error', () => reject(new Error(`Native smoke port ${port} is occupied`)));
      socket.listen(port, '127.0.0.1', () => socket.close(resolve));
    });
  }
  await mkdir(jobs, { recursive: true, mode: 0o700 });
  await rm(`${jobs}/ready.json`, { force: true });
  await cp(`${root}/scripts/treer-lab/managed-driver.mjs`, `${workspace}/managed-driver.mjs`);
  start(`${pg}/usr/lib/postgresql/18/bin/postgres`, ['-D', `${stateDir}/native-postgres-data`,
    '-h', '127.0.0.1', '-p', '15432', '-k', stateDir], {
    LD_LIBRARY_PATH: `${pg}/usr/lib/x86_64-linux-gnu`,
  });
  const adminPassword = randomBytes(32).toString('base64url');
  start(`${bins}/treer-proxy`, ['--listen', `127.0.0.1:${proxyPort}`, '--public-url', `http://127.0.0.1:${proxyPort}/`,
    '--app-public-url', `http://127.0.0.1:${webPort}/`], {
    ADMIN_PASSWORD: adminPassword, DATABASE_URL: 'postgres://treer@127.0.0.1:15432/postgres', TREER_ENABLE_CORE_MESSAGES: 'true',
  });
  await pass('auth enabled: unauthenticated user/admin rejected', async () => {
    await until('Proxy', () => api('/api/health'));
    for (const route of ['/api/auth/me', '/api/admin/me']) {
      assert.equal((await fetch(`http://127.0.0.1:${proxyPort}${route}`)).status, 401);
    }
  });
  await api('/api/admin/login', { password: adminPassword });
  const invitation = await api('/api/admin/invitations', {});
  const password = randomBytes(32).toString('base64url');
  const email = `native-${randomUUID()}@treer.invalid`;
  let user;
  await pass('real admin invitation and user registration', async () => {
    await api('/api/auth/register', { invite: invitation.token, email, preferred_name: 'Native lab user', password });
    user = await api('/api/auth/me'); assert.ok(user.user_id);
  });
  const { organization } = await api('/api/organizations', { name: 'Native Treer lab' });
  const { workspace: ws } = await api('/api/workspaces', { organization_id: organization.organization_id, name: 'Native lab' });
  const base = `/api/workspaces/${ws.workspace_id}`;
  const bootstrap = await api(`${base}/bootstrap`, {});
  const hostEnv = { XDG_CONFIG_HOME: `${stateDir}/native-config`, TREER_STATE_DIR: `${stateDir}/native-state`,
    TREER_NETWORK_MODE: 'plain', PATH: `${bins}:${process.env.PATH}`, TREER_ENROLLMENT_KEY: bootstrap.enrollment_key };
  start(`${bins}/treer-agent-server`, ['connect', '--proxy', `http://127.0.0.1:${proxyPort}/`, '--root', workspace,
    '--listen', '127.0.0.1:18790', '--name', 'native-lab-device', '--non-interactive', '--accept-risk', '--service-mode', 'foreground'], hostEnv, workspace);
  let machine;
  await pass('native fallback Host/Controller enrolled and online', async () => {
    machine = await until('native enrollment', async () => (await api(`${base}/snapshot`)).servers.find(s => s.status === 'online'));
    assert.equal(machine.root, workspace);
  });
  const terminalName = `native-shell-${randomUUID().slice(0, 8)}`;
  await api(`${base}/agents`, { server_id: machine.server_id, name: terminalName, kind: 'command', cwd: '.',
    args: ['/bin/bash', '--noprofile', '--norc'] });
  const terminal = await until('shell', async () => (await api(`${base}/snapshot`)).agents.find(a => a.name === terminalName));
  await pass('plain terminal Agent executes input', async () => {
    await api(`${base}/agents/${terminal.agent_id}/input`, { data: [...Buffer.from("printf 'LAB_%s\\n' 'NATIVE_OK'\n")] });
    await until('shell output', async () => (await api(`${base}/agents/${terminal.agent_id}/output?lines=80`)).text.includes('LAB_NATIVE_OK'));
  });
  const workerName = `native-worker-${randomUUID().slice(0, 8)}`;
  await api(`${base}/agents`, { server_id: machine.server_id, name: workerName, kind: 'command', cwd: '.',
    args: ['/usr/bin/env', `TREER_LAB_JOB_DIR=${jobs}`, `TREER_LAB_CLI=${bins}/treer`, process.execPath, `${workspace}/managed-driver.mjs`] });
  const worker = await until('managed worker', async () => {
    try { return JSON.parse(await readFile(`${jobs}/ready.json`, 'utf8')); } catch { return false; }
  });
  async function managed(args) {
    const id = randomUUID();
    await writeFile(`${jobs}/${id}.request.json`, JSON.stringify(args), { mode: 0o600 });
    const response = await until('managed reply', async () => {
      try { return JSON.parse(await readFile(`${jobs}/${id}.reply.json`, 'utf8')); } catch { return false; }
    });
    assert.equal(response.code, 0, `treer ${args.slice(0, 3).join(' ')} failed`);
    return response.value;
  }
  await pass('managed CLI identity and AIS contract', async () => {
    const identity = await managed(['whoami']); assert.equal(identity.agent.agent_id, worker.agentId);
    const contract = await managed(['interface', 'contract']); assert.equal(contract.protocol, 'treer.agent-interface/v1');
  });
  const hubScript = `${workspace}/hub.sh`;
  // Pass paths as argv rather than embedding them in shell source.
  await writeFile(hubScript, '#!/bin/sh\nset -eu\nexport POCKYMOE_MODE=local POCKYMOE_HOST_MODE=hosted POCKYMOE_HOSTED_BY=treer POCKYMOE_E2E_FAKE_RUNTIME=1\nexport HOST=127.0.0.1 PORT=18880\nexport POCKYMOE_DATABASE_PATH="$1/hub.sqlite" POCKYMOE_WORKSPACE_ROOT="$1"\nexport POCKYMOE_WEB_DIST_DIR="$2"\nexec "$3" supervisor\n', { mode: 0o700 });
  await pass('Pockymoe fake hub created as Managed App by managed Agent', async () => {
    await managed(['app', 'create', '--machine', machine.server_id, '--name', 'native-pockymoe-hub',
      '--port', '18880', hubScript, '--', workspace, `${stateDir}/build/web`, `${stateDir}/build/bin/pockymoe`]);
    await until('hub health', async () => (await fetch('http://127.0.0.1:18880/healthz')).ok);
  });
  await pass('authenticated user session can read snapshot; no anonymous workspace access', async () => {
    assert.ok((await api(`${base}/snapshot`)).agents.length >= 3);
    assert.equal((await fetch(`http://127.0.0.1:${proxyPort}${base}/snapshot`)).status, 401);
  });
  for (const target of [terminal.agent_id, worker.agentId]) await api(`${base}/agents/${target}/stop`, {});
  await save('native-evidence.json', { checks, userId: user.user_id, workspaceId: ws.workspace_id,
    dockerDevice: 'not exercised by native fallback', recordedAt: new Date().toISOString() });
} catch (error) {
  console.error(error.message);
  await save('native-evidence.json', { checks, failure: error.message, recordedAt: new Date().toISOString() });
  process.exitCode = 1;
} finally {
  for (const child of children.reverse()) { try { process.kill(-child.pid, 'SIGTERM'); } catch {} }
}
