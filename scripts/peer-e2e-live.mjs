// Cross-device peer acceptance (docs/cross-device-peer.zh.md §15): a relay and two
// fake-runtime supervisors owned by one user, plus a third device of another user.
// Real binaries, loopback ports and a temporary HOME; nothing on the host is touched.
// Usage: cargo build -p pockymoe && node scripts/peer-e2e-live.mjs [path/to/pockymoe]
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { spawn, execFile } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const binary = resolve(process.argv[2] ?? 'target/debug/pockymoe');
assert(existsSync(binary), `missing ${binary}; run cargo build -p pockymoe`);
const root = mkdtempSync(join(tmpdir(), 'peer-e2e-'));
if (process.env.PEER_E2E_KEEP) console.log(JSON.stringify({ stage: 'root', root }));
const password = randomBytes(24).toString('hex');
const children = new Map();
const sleep = ms => new Promise(r => setTimeout(r, ms));
const sha256 = data => createHash('sha256').update(data).digest('hex');
const freePort = () => new Promise(done => {
  const server = createServer().listen(0, '127.0.0.1', () => {
    const { port } = server.address();
    server.close(() => done(port));
  });
});
const baseEnv = {
  PATH: process.env.PATH, HOME: join(root, 'home'), HOST: '127.0.0.1',
  POCKYMOE_E2E_FAKE_RUNTIME: '1',
  POCKYMOE_ADMIN_USERNAME: 'testadmin', POCKYMOE_ADMIN_PASSWORD: password,
  POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
  POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
};
mkdirSync(baseEnv.HOME, { recursive: true });

function start(name, mode, extra) {
  const child = spawn(binary, [mode], { stdio: ['ignore', 'ignore', 'pipe'], env: { ...baseEnv, ...extra } });
  let log = '';
  child.stderr.on('data', chunk => { log = (log + chunk).slice(-20000); });
  child.log = () => log;
  children.set(name, child);
  return child;
}
async function stop(name) {
  const child = children.get(name);
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exit = once(child, 'exit');
  child.kill('SIGTERM');
  await exit;
}
async function until(fn, timeout = 15000, what = 'condition') {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try { const value = await fn(); if (value) return value; } catch (error) { last = error; }
    await sleep(150);
  }
  assert.fail(`${what} timed out${last ? `: ${last.message}` : ''}`);
}

const relayPort = await freePort();
const relay = `http://127.0.0.1:${relayPort}`;
async function api(token, path, body, method) {
  const response = await fetch(`${relay}${path}`, {
    method: method ?? (body ? 'POST' : 'GET'),
    headers: { ...(body && !(body instanceof FormData) ? { 'content-type': 'application/json' } : {}), ...(token ? { authorization: `Bearer ${token}` } : {}) },
    ...(body ? { body: body instanceof FormData ? body : JSON.stringify(body) } : {}),
    signal: AbortSignal.timeout(60000),
  });
  const text = await response.text();
  assert(response.ok, `HTTP ${response.status} at ${path}: ${text.slice(0, 300)}`);
  return text ? JSON.parse(text) : null;
}
async function account(username) {
  await api(null, '/relay/auth/register', { username, email: `${username}@example.test`, password });
  return (await api(null, '/relay/auth/login', { username, password })).token;
}
async function device(token, name) {
  const created = await api(token, '/relay/devices', { name });
  const port = await freePort();
  const db = join(root, `${name}.sqlite`);
  const spec = {
    name, id: created.device.id, token, prefix: `/relay/devices/${created.device.id}`,
    cli: db.replace(/\.sqlite$/, '.cli.json'), workspace: join(root, `${name}-workspace`),
    env: {
      PORT: String(port), POCKYMOE_RELAY_SUPERVISOR_PORT: String(port),
      POCKYMOE_RELAY_SERVER_URL: relay, POCKYMOE_RELAY_AGENT_TOKEN: created.token,
      POCKYMOE_DATABASE_PATH: db, POCKYMOE_WORKSPACE_ROOT: join(root, `${name}-workspaces`),
    },
  };
  mkdirSync(spec.workspace, { recursive: true });
  return spec;
}
async function boot(spec) {
  start(spec.name, 'relay-supervisor', spec.env);
  await until(async () => (await api(spec.token, `${spec.prefix}/presence`)).connected, 20000, `${spec.name} presence`);
  await until(() => existsSync(spec.cli), 10000, `${spec.name} cli config`);
}
const owner = (spec, path, body, method) => api(spec.token, `${spec.prefix}${path}`, body, method);

function rc(spec, args, { from, allowFailure = false } = {}) {
  const argv = ['--cli-config', spec.cli, ...(from ? ['--from', from] : []), ...args];
  return new Promise(done => execFile(binary, argv, { env: { ...baseEnv }, maxBuffer: 64 << 20 }, (error, stdout, stderr) => {
    let json = null;
    try { json = JSON.parse(stdout); } catch { /* text output */ }
    if (error && !allowFailure) assert.fail(`pockymoe ${args.join(' ')} failed: ${stderr || stdout}`);
    done({ ok: !error, json, stdout, stderr });
  }));
}
const step = (stage, detail = {}) => console.log(JSON.stringify({ stage, ...detail }));

try {
  start('relay', 'relay', { PORT: String(relayPort), POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'), POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true' });
  await until(() => api(null, '/healthz'), 20000, 'relay health');
  const ownerToken = await account('owner');
  const otherToken = await account('stranger');
  const alpha = await device(ownerToken, 'alpha');
  const beta = await device(ownerToken, 'beta');
  const gamma = await device(otherToken, 'gamma');
  for (const spec of [alpha, beta, gamma]) await boot(spec);
  step('booted', { alpha: alpha.id, beta: beta.id, gamma: gamma.id });

  const workspaceA = await owner(alpha, '/api/workspaces', { label: 'alpha', absPath: alpha.workspace });
  const workspaceB = await owner(beta, '/api/workspaces', { label: 'beta', absPath: beta.workspace });
  const threadId = created => created.thread?.id ?? created.id;
  const caller = threadId(await owner(alpha, '/api/threads/start', { workspaceId: workspaceA.id, model: 'fake' }));
  const target = threadId(await owner(beta, '/api/threads/start', { workspaceId: workspaceB.id, model: 'fake' }));

  // Directory and identity.
  const info = (await rc(alpha, ['thread', 'self'], { from: caller })).json;
  assert(info, 'thread self returns JSON');
  const devices = (await rc(alpha, ['device', 'list'])).json;
  const rows = devices.devices ?? devices;
  assert(rows.some(d => d.deviceId === alpha.id && d.self), 'directory marks this device');
  assert(rows.some(d => d.deviceId === beta.id && d.online), 'directory lists the other device online');
  assert(!rows.some(d => d.deviceId === gamma.id), 'directory never lists another owner');
  step('directory', { devices: rows.length });

  // Only the target opts in. Alpha keeps peer access off for the whole run, so
  // beta's replies and completion notifications below come in as answers.
  const offTarget = await rc(alpha, ['thread', 'list', '--device', 'beta'], { allowFailure: true });
  assert(!offTarget.ok && /peer_access_disabled|disabled/i.test(offTarget.stderr + offTarget.stdout), 'target opt-in required');
  await owner(beta, '/api/config/peer-access', { enabled: true }, 'PATCH');
  const unsolicited = await rc(beta, ['thread', 'list', '--device', 'alpha'], { allowFailure: true });
  assert(!unsolicited.ok && /peer_access_disabled|disabled/i.test(unsolicited.stderr + unsolicited.stdout), 'caller with access off takes only answers');
  const listed = (await rc(alpha, ['thread', 'list', '--device', 'beta'])).json;
  assert(JSON.stringify(listed).includes(target), 'remote list shows the target thread');
  step('opt-in');

  // Another owner's device is indistinguishable from a missing one.
  const stranger = await rc(alpha, ['thread', 'list', '--device', gamma.id], { allowFailure: true });
  assert(!stranger.ok && /not.found|unknown device|device_not_found/i.test(stranger.stderr + stranger.stdout), 'other owner is not reachable');
  step('owner-boundary');

  // Status, passive mail with remote attribution, and URL addressing.
  const status = (await rc(alpha, ['thread', 'status', `beta/${target}`])).json;
  assert.equal(status.threadId ?? status.id, target);
  const mail = (await rc(alpha, ['thread', 'send', `beta/${target}`, '--text', 'hello from alpha', '--subject', 'greeting'], { from: caller })).json;
  assert.equal(mail.delivery, 'inbox');
  const inboxB = (await rc(beta, ['inbox', '--thread', target])).json;
  const greeting = inboxB.messages.find(m => m.subject === 'greeting');
  assert(greeting && greeting.fromDeviceId === alpha.id && greeting.replyTo === `${alpha.id}/${caller}`, 'remote sender recorded');
  const viaUrl = (await rc(alpha, ['thread', 'status', `https://example.test/devices/${beta.id}/threads/${target}`])).json;
  assert.equal(viaUrl.threadId ?? viaUrl.id, target);
  const selfUrl = (await rc(alpha, ['thread', 'status', `https://example.test/devices/${alpha.id}/threads/${caller}`])).json;
  assert.equal(selfUrl.threadId ?? selfUrl.id, caller, 'a URL for this device stays local');
  step('mail');
  // The receiver answers through the address its mail carried.
  await rc(beta, ['thread', 'send', greeting.replyTo, '--text', 'reply from beta', '--subject', 'reply', '--in-reply-to', greeting.id], { from: target });
  const reply = (await rc(alpha, ['inbox', '--thread', caller])).json.messages.find(m => m.subject === 'reply');
  assert(reply && reply.fromDeviceId === beta.id && reply.inReplyTo === greeting.id, 'reply routed back with attribution');
  step('reply');

  // Execution with a completion notification delivered back to the caller's inbox.
  await rc(alpha, ['thread', 'send', `beta/${target}`, '--delivery', 'queue', '--kind', 'task', '--notify-on-complete', '--text', 'run the remote task'], { from: caller });
  const result = await until(async () => {
    const inbox = (await rc(alpha, ['inbox', '--thread', caller])).json;
    return inbox.messages.find(m => m.kind === 'result' && m.fromDeviceId === beta.id);
  }, 60000, 'remote completion notification');
  const transcript = (await rc(alpha, ['transcript', `beta/${target}`])).json;
  assert(JSON.stringify(transcript).includes('run the remote task'), 'remote transcript shows the prompt');
  step('notify-on-complete', { message: result.id });

  // Remote creation.
  const createdRemote = (await rc(alpha, ['thread', 'create', '--device', 'beta', '--workspace', workspaceB.id, '--provider', 'codex', '--title', 'remote helper', '--text', 'start remotely'], { from: caller })).json;
  assert(createdRemote.threadId, 'remote create returns the new thread');
  step('remote-create', { thread: createdRemote.threadId });

  // Attachments: text, a directory, and a binary larger than one 16 MiB frame.
  const files = join(root, 'outgoing');
  mkdirSync(join(files, 'docs'), { recursive: true });
  writeFileSync(join(files, 'notes.md'), '# notes\n');
  writeFileSync(join(files, 'docs', 'a.txt'), 'a\n');
  const big = randomBytes(20 * 1024 * 1024);
  writeFileSync(join(files, 'big.bin'), big);
  const sent = (await rc(alpha, ['thread', 'send', `beta/${target}`, '--text', 'files attached', '--subject', 'files',
    '--attach', join(files, 'notes.md'), '--attach', join(files, 'docs'), '--attach', join(files, 'big.bin')], { from: caller })).json;
  const attachments = sent.attachments ?? [];
  assert.equal(attachments.length, 3, 'three attachments committed');
  const bigCopy = attachments.find(a => a.name === 'big.bin');
  assert(bigCopy && bigCopy.sha256 === sha256(big) && existsSync(bigCopy.path), 'big attachment verified on the target');
  assert.equal(sha256(readFileSync(bigCopy.path)), sha256(big));
  assert(attachments.some(a => a.name.endsWith('.zip')), 'directory travels as a zip');
  step('attachments', { names: attachments.map(a => a.name) });

  // Read-only pull from the remote workspace.
  writeFileSync(join(beta.workspace, 'report.txt'), 'remote report\n');
  const listing = (await rc(alpha, ['fs', 'ls', 'beta', '--workspace', workspaceB.id])).json;
  assert(JSON.stringify(listing).includes('report.txt'), 'remote listing');
  const out = join(root, 'pulled.txt');
  await rc(alpha, ['fs', 'get', 'beta', '--workspace', workspaceB.id, 'report.txt', '--out', out]);
  assert.equal(readFileSync(out, 'utf8'), 'remote report\n');
  const escape = await rc(alpha, ['fs', 'get', 'beta', '--workspace', workspaceB.id, '../alpha.sqlite', '--out', join(root, 'x')], { allowFailure: true });
  assert(!escape.ok, 'workspace confinement');
  step('fs');

  // Offline target: inbox mail waits in the caller's outbox and is delivered later.
  await stop('beta');
  await until(async () => !(await api(beta.token, `${beta.prefix}/presence`)).connected, 15000, 'beta offline');
  const queued = (await rc(alpha, ['thread', 'send', `beta/${target}`, '--text', 'sent while offline', '--subject', 'offline'], { from: caller })).json;
  assert.equal(queued.delivery, 'outboxed');
  assert((await rc(alpha, ['outbox'])).stdout.includes(queued.outboxId), 'outbox lists the pending message');
  await boot(beta);
  await until(async () => {
    const inbox = (await rc(beta, ['inbox', '--thread', target])).json;
    return inbox.messages.some(m => m.subject === 'offline');
  }, 45000, 'outbox delivery after reconnect');
  step('outbox');

  // A browser-style upload above one default WebSocket frame no longer drops the tunnel.
  const before = (await owner(beta, '/healthz')).processId;
  const form = new FormData();
  form.append('file', new Blob([randomBytes(20 * 1024 * 1024)]), 'large.bin');
  await owner(beta, `/api/workspaces/${workspaceB.id}/files/upload`, form);
  assert((await api(beta.token, `${beta.prefix}/presence`)).connected, 'tunnel survived');
  assert.equal((await owner(beta, '/healthz')).processId, before);
  step('large-upload');
  console.log(JSON.stringify({ ok: true }));
} catch (error) {
  for (const [name, child] of children) console.error(`--- ${name} stderr tail ---\n${child.log?.() ?? ''}`);
  throw error;
} finally {
  for (const name of [...children.keys()].reverse()) await stop(name);
  if (!process.env.PEER_E2E_KEEP) rmSync(root, { recursive: true, force: true });
}
