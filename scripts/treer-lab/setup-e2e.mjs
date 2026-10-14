import assert from 'node:assert/strict';
import { randomUUID, createHash } from 'node:crypto';
import { readFile, copyFile } from 'node:fs/promises';
import { root, stateDir, treerSource, project, read, save, api, until, deviceExec,
  docker, compose, quote, run } from './common.mjs';

// Preparation is explicit: real gh release assets under setup-mirror/v0.13.2,
// Python 3.11+ and curl installed in the running device. Never substitutes a build.
const checks = [], setupRuns = [];
const evidence = { version: '0.13.2', checks, setupRuns };
const mirrorName = `${project}-setup-mirror`;
const mirrorRoot = `${stateDir}/setup-mirror`;
let mirrorStarted = false, forwardPid, ws;
async function check(name, fn) {
  try { await fn(); checks.push({ name, result: 'passed' }); console.log(`PASS ${name}`); }
  catch (error) { checks.push({ name, result: 'failed', failure: error.message }); throw error; }
}
function appSummary(app) {
  return { id: app.app_id, name: app.name, port: app.port, pid: app.pid,
    status: app.status, desiredState: app.desired_state, command: app.command, args: app.args, cwd: app.cwd };
}
try {
  const state = await read(); ws = `/api/workspaces/${state.workspaceId}`;
  const snapshot = () => api(`${ws}/snapshot`);
  const apps = async () => (await api(`${ws}/apps`)).apps;
  const profiles = async () => (await api(`${ws}/launch-profiles`)).profiles;
  assert.equal(state.authEnabled, true);
  await api('/api/auth/me');
  const beforeSnapshot = await snapshot();
  const machine = beforeSnapshot.servers.find(s => s.name === 'lab-device' && s.status === 'online');
  assert.ok(machine);
  const oldAgents = beforeSnapshot.agents.filter(a => a.interface && a.status !== 'exited');
  const oldApps = await apps();
  const oldProfiles = await profiles();
  evidence.baseline = { agents: oldAgents.map(a => ({ id: a.agent_id, name: a.name,
    pid: a.pid, interface: a.interface })), apps: oldApps.map(appSummary), profileCount: oldProfiles.length };
  await check('Python/curl prerequisites and genuine pinned release checksums', async () => {
    evidence.python = (await deviceExec(['python3', '--version'])).trim();
    await deviceExec(['python3', '-c', 'import sys; assert sys.version_info >= (3,11)']);
    await deviceExec(['curl', '--version']);
    const sums = new Map((await readFile(`${mirrorRoot}/v0.13.2/SHA256SUMS`, 'utf8'))
      .trim().split('\n').map(line => { const [sha, name] = line.trim().split(/\s+/); return [name.replace(/^\*/, ''), sha]; }));
    evidence.assets = [];
    for (const name of ['remote-codex-linux-x64-gnu', 'remote-codex-web.zip', 'runtime-version.txt']) {
      const data = await readFile(`${mirrorRoot}/v0.13.2/${name}`);
      const sha256 = createHash('sha256').update(data).digest('hex');
      assert.equal(sha256, sums.get(name)); evidence.assets.push({ name, sha256, bytes: data.length });
    }
    assert.equal((await readFile(`${mirrorRoot}/v0.13.2/runtime-version.txt`, 'utf8')).trim(), '0.13.2');
  });
  // Auxiliary container has no published ports; the existing device stays running.
  const deviceId = (await compose(['ps', '-q', 'device'])).trim();
  evidence.deviceContainerId = deviceId;
  const networks = JSON.parse(await docker(['inspect', '--format', '{{json .NetworkSettings.Networks}}', deviceId]));
  assert.ok(networks[`${project}_default`]);
  await copyFile(`${root}/integrations/treer/treer-setup.sh`, `${mirrorRoot}/treer-setup.sh`);
  evidence.setupSha256 = createHash('sha256').update(await readFile(`${mirrorRoot}/treer-setup.sh`)).digest('hex');
  await docker(['run', '-d', '--rm', '--name', mirrorName, '--network', `${project}_default`,
    '--user', '0', '--entrypoint', '/usr/local/bin/node',
    '-v', `${mirrorRoot}:/mirror:ro`, '-v', `${root}/scripts/treer-lab/setup-mirror.mjs:/setup-mirror.mjs:ro`,
    `${project}:device`, '/setup-mirror.mjs']);
  mirrorStarted = true;
  await until('setup HTTP mirror', async () => {
    try { await deviceExec(['curl', '-fsS', `http://${mirrorName}:19000/v0.13.2/runtime-version.txt`]); return true; }
    catch { return false; }
  });
  // The documented dev mirror accepts loopback HTTP only; no script changes needed.
  forwardPid = (await deviceExec(['sh', '-c',
    `socat TCP-LISTEN:19000,bind=127.0.0.1,reuseaddr,fork TCP:${mirrorName}:19000 >/dev/null 2>&1 </dev/null & printf '%s' "$!"`])).trim();
  assert.match(forwardPid, /^\d+$/);
  await until('loopback release forward', async () => {
    try { await deviceExec(['curl', '-fsS', 'http://127.0.0.1:19000/v0.13.2/runtime-version.txt']); return true; }
    catch { return false; }
  });
  const command = `curl -fsSL http://${mirrorName}:19000/treer-setup.sh -o treer-setup.sh && sh treer-setup.sh --version 0.13.2`;
  evidence.command = command;
  evidence.runOnlyEnvironment = { POCKYMOE_RELEASE_BASE_URL: 'http://127.0.0.1:19000' };
  async function setupRun(number) {
    const name = `lab-setup-${number}-${randomUUID().slice(0, 8)}`;
    const marker = `LAB_SETUP_EXIT_${number}=`;
    await api(`${ws}/agents`, { server_id: machine.server_id, name, kind: 'command', cwd: 'project',
      args: ['/bin/sh', '-c', `export POCKYMOE_RELEASE_BASE_URL=http://127.0.0.1:19000\n${command}\nsetup_status=$?\nprintf '\\n${marker}%s\\n' "$setup_status"\nexit "$setup_status"`] });
    const agent = await until('plain setup Agent', async () => (await snapshot()).agents.find(a => a.name === name));
    const output = await until('setup exit output', async () => {
      const out = await api(`${ws}/agents/${agent.agent_id}/output?lines=300`);
      return out.text.includes(marker) && out;
    }, 180_000);
    const completed = await until('setup Agent exited', async () => {
      const found = (await snapshot()).agents.find(a => a.agent_id === agent.agent_id);
      return found.status === 'exited' && found;
    });
    const text = output.text.replaceAll('\r', '').trim();
    const exit = Number(text.match(new RegExp(`${marker}(\\d+)`))[1]);
    const result = { number, agentId: agent.agent_id, exit, treerExitCode: completed.exit_code, output: text };
    setupRuns.push(result); await save('setup-evidence.json', evidence);
    console.log(text);
    assert.equal(exit, 0); assert.equal(completed.exit_code, 0);
    assert.ok(text.includes('AIS v1 provenance and prompt-delivery contract verified.'));
    return result;
  }
  let hub, firstProfiles;
  await check('First exact one-command setup exits zero inside a plain Treer Agent', () => setupRun(1));
  await check('Native layout, running Managed App and three stable launch profiles', async () => {
    assert.equal((await deviceExec(['/home/lab/.local/bin/pockymoe', 'version'])).trim(), '0.13.2');
    evidence.nativeCurrent = (await deviceExec(['readlink', '-f', '/home/lab/.local/share/remote-codex/native/current'])).trim();
    assert.equal(evidence.nativeCurrent, '/home/lab/.local/share/remote-codex/native/releases/0.13.2');
    hub = await until('Managed App running and healthy', async () => {
      const app = (await apps()).find(a => a.name === 'pockymoe');
      if (!app || app.status !== 'running') return false;
      await deviceExec(['curl', '-fsS', `http://127.0.0.1:${app.port}/healthz`]); return app;
    });
    assert.notEqual(hub.port, 18880); assert.notEqual(hub.port, 8787);
    evidence.hubHealth = JSON.parse(await deviceExec(['curl', '-fsS', `http://127.0.0.1:${hub.port}/healthz`]));
    evidence.hub = appSummary(hub);
    assert.equal(hub.command, '/usr/bin/env');
    assert.ok(hub.args.includes('/home/lab/.local/bin/pockymoe'));
    assert.ok(hub.args.includes('POCKYMOE_WEB_DIST_DIR=/home/lab/.local/share/remote-codex/native/current/web'));
    assert.ok(hub.args.every(arg => !arg.includes('/releases/')));
    firstProfiles = (await profiles()).filter(p => ['Pockymoe Codex', 'Pockymoe Claude', 'Pockymoe Grok'].includes(p.name));
    assert.equal(firstProfiles.length, 3);
    for (const p of firstProfiles) {
      assert.equal(p.command, '/home/lab/.local/bin/pockymoe');
      assert.ok(p.args.includes(`http://127.0.0.1:${hub.port}`));
      assert.ok(p.args.every(arg => !arg.includes('/releases/')));
    }
    evidence.profiles = firstProfiles;
    evidence.installedBinarySha256 = (await deviceExec(['sha256sum', '/home/lab/.local/bin/pockymoe'])).trim().split(/\s+/)[0];
    assert.equal(evidence.installedBinarySha256, evidence.assets.find(a => a.name === 'remote-codex-linux-x64-gnu').sha256);
  });
  await check('Same command rerun exits zero with no duplicate App or profiles', async () => {
    await setupRun(2);
    const nowApps = await apps(), nowProfiles = await profiles();
    const sameHub = nowApps.filter(a => a.name === 'pockymoe');
    assert.equal(sameHub.length, 1); assert.equal(sameHub[0].app_id, hub.app_id);
    assert.equal(sameHub[0].pid, hub.pid); assert.equal(sameHub[0].port, hub.port);
    assert.equal(nowApps.length, oldApps.length + 1);
    for (const p of firstProfiles) {
      const matches = nowProfiles.filter(n => n.name === p.name);
      assert.equal(matches.length, 1); assert.equal(matches[0].profile_id, p.profile_id);
      assert.deepEqual(matches[0].args, p.args);
    }
    assert.equal(nowProfiles.length, oldProfiles.length + 3);
  });
  await check('Existing lab App and Agents remain alive with unchanged identities and ports', async () => {
    const current = await snapshot(); const currentApps = await apps();
    for (const old of oldApps) {
      const now = currentApps.find(a => a.app_id === old.app_id);
      assert.equal(now.pid, old.pid); assert.equal(now.port, old.port); assert.equal(now.status, 'running');
      await deviceExec(['curl', '-fsS', `http://127.0.0.1:${old.port}/healthz`]);
    }
    for (const old of oldAgents) {
      const now = current.agents.find(a => a.agent_id === old.agent_id);
      assert.equal(now.pid, old.pid); assert.deepEqual(now.interface, old.interface);
      const data = await api(`${ws}/agents/${old.agent_id}/interface/ui/api/threads`);
      assert.ok((Array.isArray(data) ? data : data.threads).length > 0);
      await api(`${ws}/agents/${old.agent_id}/transcript?page=0&limit=1`);
    }
    evidence.finalApps = currentApps.map(appSummary);
  });
  let attached;
  await check('Created Codex profile launches with AIS v1 and all seven hub capabilities', async () => {
    const profile = firstProfiles.find(p => p.name === 'Pockymoe Codex');
    const name = `lab-setup-codex-${randomUUID().slice(0, 8)}`;
    await api(`${ws}/launch-profiles/${profile.profile_id}/launch`, {
      server_id: machine.server_id, agent_name: name, cwd: 'project', cols: 120, rows: 36 });
    attached = await until('setup profile AIS interface', async () => {
      const a = (await snapshot()).agents.find(a => a.name === name); return (a?.interface || a?.status === 'exited') && a;
    }, 90_000);
    if (!attached.interface) {
      evidence.attachAttempt = { id: attached.agent_id, name, pid: attached.pid,
        status: attached.status, exitCode: attached.exit_code, interface: null,
        output: (await api(`${ws}/agents/${attached.agent_id}/output?lines=40`)).text.replaceAll('\r', '').trim() };
      throw new Error(`Created profile Agent exited ${attached.exit_code} before registering its interface`);
    }
    assert.equal(attached.interface.protocol, 'treer.agent-interface/v1');
    assert.equal(attached.interface.port, hub.port);
    assert.deepEqual([...attached.interface.capabilities].sort(),
      ['prompt.submit', 'prompt.queue', 'prompt.queue.read', 'prompt.notify', 'transcript.read', 'state.observe', 'abort'].sort());
    evidence.attachAgent = { id: attached.agent_id, name, pid: attached.pid, interface: attached.interface };
  });
  const scoped = await api(`${ws}/agents/${attached.agent_id}/interface/ui/api/threads`);
  const threads = Array.isArray(scoped) ? scoped : scoped.threads;
  assert.equal(threads.length, 1); const threadId = threads[0].id;
  evidence.threadId = threadId;
  // This is an observation, not a setup success gate. The lab has no real Codex adapter.
  const promptText = 'Treer one-command setup smoke prompt';
  evidence.prompt = { text: promptText, codexInstalled: false };
  await deviceExec(['sh', '-c', '! command -v codex && ! command -v codex-acp']);
  const receipt = await api(`${ws}/agents/${attached.agent_id}/prompt`, { text: promptText });
  evidence.prompt.receipt = receipt;
  const detail = await until('first prompt terminal outcome', async () => {
    const data = await api(`${ws}/agents/${attached.agent_id}/interface/ui/api/threads/${threadId}`);
    return data.turns?.some(t => ['failed', 'completed', 'interrupted'].includes(t.status)) && data;
  }, 90_000);
  evidence.prompt.turns = detail.turns.map(t => ({ id: t.id, status: t.status, error: t.error }));
  evidence.prompt.agentStatus = (await snapshot()).agents.find(a => a.agent_id === attached.agent_id)?.status;
  console.log(`OBSERVE first prompt: ${evidence.prompt.turns.map(t => t.status).join(', ')}`);
  await save('setup-browser.json', { workspaceId: state.workspaceId, agentId: attached.agent_id,
    threadId, promptText });
  await check('Authenticated Chromium Agent tunnel loads its bound thread page', async () => {
    await run(process.execPath, [`${treerSource}/web/node_modules/@playwright/test/cli.js`, 'test',
      'setup-tunnel.spec.mjs', '--config', `${root}/scripts/treer-lab/playwright.config.mjs`, '--project=desktop-chromium'],
    { inherit: true });
  });

} catch (error) {
  evidence.failure = error.message;
  if (evidence.attachAttempt?.interface === null) {
    checks.push({ name: 'Authenticated Chromium Agent tunnel loads its bound thread page',
      result: 'blocked', reason: 'No bound thread or AIS interface; Playwright not executed' });
    checks.push({ name: 'First real Codex prompt observation', result: 'blocked',
      reason: 'No bound thread or AIS prompt capability exists' });
  }
  console.error(`Setup lab verification failed: ${error.message}`);
  process.exitCode = 1;
} finally {
  if (forwardPid) await deviceExec(['kill', forwardPid]).catch(() => {});
  if (mirrorStarted) await docker(['stop', mirrorName]).catch(() => {});
  evidence.recordedAt = new Date().toISOString();
  await save('setup-evidence.json', evidence);
}
