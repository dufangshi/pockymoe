// Treer Issues App against bound Pockymoe threads. Run from the lab worktree after
// `e2e.mjs --full`; see README "Treer Issues App". Evidence: .scratch/treer-issues/.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { root, read, api, until, deviceExec, docker, compose, requireDocker, quote } from './common.mjs';

const binary = process.env.TREER_LAB_POCKYMOE_BIN;
const issuesSource = process.env.TREER_LAB_ISSUES_SOURCE ?? '/home/ubuntu/dev/remoteCodex/reference/treer/apps/issues';
const labScripts = process.env.TREER_LAB_SCRIPTS ?? `${root}/scripts/treer-lab`;
const evidenceDir = process.env.TREER_LAB_ISSUES_EVIDENCE_DIR ?? `${root}/.scratch/treer-issues`;
const hub = 'http://127.0.0.1:18880';
const checks = [];
async function check(name, fn) {
  try { await fn(); } catch (error) { checks.push({ name, result: 'failed', failure: error.message }); throw error; }
  checks.push({ name, result: 'passed' });
  console.log(`PASS ${name}`);
}
const asRoot = (args, options) => compose(['ps', '-q', 'device']).then(id =>
  docker(['exec', '-u', '0', '-i', id.trim(), ...args], options));
async function job(dir, body, timeout = 90_000) {
  const id = randomUUID();
  await deviceExec(['/bin/sh', '-c', 'umask 077; cat > "$1.tmp" && mv "$1.tmp" "$1"', 'sh', `${dir}/${id}.request.json`],
    { input: JSON.stringify(body) });
  return until(`job ${body.cli ?? ''} ${(body.args ?? body).slice(0, 2).join(' ')}`, async () => {
    try { return JSON.parse(await deviceExec(['cat', `${dir}/${id}.reply.json`])); } catch { return false; }
  }, timeout);
}
const managed = async args => {
  const reply = await job('/workspace/lab-jobs', args);
  assert.equal(reply.code, 0, `managed treer ${args.slice(0, 3).join(' ')} failed`);
  return reply.value;
};
const thread = t => async (cli, args) => job(`/workspace/l2-thread-jobs/${t.thread}`, { cli, args });
const sql = (query, ...params) => deviceExec(['python3', '-c', `
import json, sqlite3, sys
c = sqlite3.connect('file:/workspace/hub/device.sqlite?mode=ro', uri=True, timeout=10)
print(json.dumps([list(r) for r in c.execute(sys.argv[1], sys.argv[2:])]))`, query, ...params]).then(JSON.parse);
const issues = (t, method, path, body) => thread(t)('curl', ['-fsS', '-X', method,
  ...(body ? ['-H', 'Content-Type: application/json', '--data', JSON.stringify(body)] : []),
  `http://issues.treer${path}`]).then(reply => { assert.equal(reply.code, 0, `${method} ${path} failed`); return reply.value; });

try {
  await requireDocker();
  const state = await read();
  const ws = `/api/workspaces/${state.workspaceId}`;
  const snapshot = () => api(`${ws}/snapshot`);
  const machine = (await snapshot()).servers.find(s => s.status === 'online' && s.name === 'lab-device');
  assert.ok(machine, 'lab device online');
  let hubApp, issuesApp;
  const threads = {};

  await check('Candidate binary, curl-capable thread fixture and Issues source installed', async () => {
    assert.ok(binary, 'TREER_LAB_POCKYMOE_BIN is required');
    const id = (await compose(['ps', '-q', 'device'])).trim();
    await docker(['cp', binary, `${id}:/usr/local/bin/pockymoe.next`]);
    for (const file of ['thread-driver.mjs', 'thread-driver.sh']) {
      await docker(['cp', `${labScripts}/${file}`, `${id}:/opt/lab/${file}`]);
    }
    // The opt-in strict fixture (README "Strict registry CLI fixture"), on the fake hub only.
    await asRoot(['sh', '-c', "chmod 755 /opt/lab/thread-driver.sh && (grep -q FAKE_CLI_DRIVER /opt/lab/hub.sh || sed -i '/POCKYMOE_E2E_FAKE_RUNTIME=1/a export POCKYMOE_E2E_FAKE_CLI_DRIVER=/opt/lab/thread-driver.sh' /opt/lab/hub.sh)"]);
    await asRoot(['rm', '-rf', '/opt/issues']);
    await docker(['cp', issuesSource, `${id}:/opt/issues`]);
    // Rename over the running executable; live processes keep their old inode.
    await asRoot(['sh', '-c', 'chmod 755 /usr/local/bin/pockymoe.next && mv /usr/local/bin/pockymoe.next /usr/local/bin/pockymoe && chmod 644 /opt/lab/thread-driver.mjs && chmod -R a+rX /opt/issues']);
    const built = (await deviceExec(['sha256sum', '/usr/local/bin/pockymoe'])).split(' ')[0];
    checks.push({ name: 'installed binary sha256', value: built });
  });

  await check('Managed caller, restarted hub App and Issues App', async () => {
    for (const old of (await snapshot()).agents.filter(a => a.name.startsWith('lab-driver-') && a.status !== 'exited')) {
      await api(`${ws}/agents/${old.agent_id}/stop`, {});
    }
    await deviceExec(['rm', '-f', '/workspace/lab-jobs/ready.json']);
    await api(`${ws}/agents`, { server_id: machine.server_id, name: `lab-driver-${randomUUID().slice(0, 8)}`,
      kind: 'command', cwd: '.', args: ['/usr/local/bin/node', '/opt/lab/managed-driver.mjs'] });
    await until('managed driver', async () => JSON.parse(await deviceExec(['cat', '/workspace/lab-jobs/ready.json'])));
    const apps = (await managed(['app', 'list'])).apps;
    hubApp = apps.find(a => a.name === 'pockymoe-lab-hub');
    assert.ok(hubApp, 'fake hub App exists');
    const pid = hubApp.pid;
    await managed(['app', 'restart', hubApp.app_id]);
    await until('hub App replaced', async () => {
      const app = (await managed(['app', 'list'])).apps.find(a => a.app_id === hubApp.app_id);
      return app.pid && app.pid !== pid && JSON.parse(await deviceExec(['curl', '-fsS', `${hub}/healthz`]));
    }, 90_000);
    issuesApp = apps.find(a => a.name === 'issues');
    if (!issuesApp) {
      await deviceExec(['mkdir', '-p', '/workspace/issues']);
      await managed(['app', 'create', '--machine', machine.server_id, '--name', 'issues', '--cwd', '.', '--port', '9440',
        '/usr/bin/env', '--', 'ISSUES_PROXY_URL=http://127.0.0.1:8787/', 'ISSUES_DB=/workspace/issues/issues.sqlite3',
        'ISSUES_REMINDER_FIRST=1m', 'ISSUES_LISTEN=127.0.0.1:9440', 'python3', '/opt/issues/issues.py']);
      issuesApp = await until('Issues App', async () => (await managed(['app', 'list'])).apps.find(a => a.name === 'issues'));
    }
    await until('Issues health', async () => JSON.parse(await deviceExec(['curl', '-fsS', 'http://127.0.0.1:9440/health'])).status === 'ok');
    assert.match(issuesApp.runtime_agent_id, /^(app|ag)_/);
  });

  await check('Two new bound threads run with their own Agent identity', async () => {
    for (const role of ['author', 'assignee']) {
      const name = `issues-${role}-${randomUUID().slice(0, 6)}`;
      const created = await managed(['agent', 'admin', 'create', '--machine', machine.server_id, '--kind', 'command',
        '--name', name, '--cwd', 'project', '--', '/usr/local/bin/pockymoe', 'attach', '--provider', 'codex',
        '--hub', hub, '--hub-token-file', '/workspace/hub/treer-attach.token']);
      const agent = created.agent?.agent_id ?? created.agent_id;
      assert.ok(agent, 'created Agent ID');
      const [[threadId]] = await until('thread bound', async () => {
        const rows = await sql('SELECT thread_id FROM treer_bindings WHERE agent_id=?', agent);
        return rows.length && rows;
      }, 60_000);
      // Attach registers the interface after binding; `busy` appears with state.observe.
      await until('interface registered', async () => typeof (await managed(['agent', 'show', agent])).busy === 'boolean', 60_000);
      // The fake runtime starts the fixture worker on the first turn.
      await managed(['agent', 'prompt', agent, 'start the fixture']);
      const ready = await until('thread fixture', async () =>
        JSON.parse(await deviceExec(['cat', `/workspace/l2-thread-jobs/${threadId}/ready.json`])), 60_000);
      assert.equal(ready.agentId, agent);
      threads[role] = { agent, thread: threadId, name };
    }
  });

  await check('A thread reaches the Issues App as its own Agent, not as the hub App', async () => {
    for (const t of Object.values(threads)) {
      const me = await issues(t, 'GET', '/v1/me');
      assert.equal(me.principal.kind, 'agent');
      assert.equal(me.principal.id, t.agent);
      assert.notEqual(me.principal.id, hubApp.runtime_agent_id);
    }
    const directory = await issues(threads.author, 'GET', '/v1/directory');
    assert.ok(directory.principals.some(p => p.id === threads.assignee.agent), 'assignee in directory');
  });

  let issue;
  const turns = t => sql('SELECT count(*) FROM thread_turns WHERE thread_id=?', t.thread).then(r => r[0][0]);
  await check('Agent-assigned issue reaches the assignee inbox without starting a turn', async () => {
    await until('assignee idle', async () => (await managed(['agent', 'show', threads.assignee.agent])).busy === false, 60_000);
    const before = await turns(threads.assignee);
    const created = await issues(threads.author, 'POST', '/v1/issues', {
      title: 'Lab: verify Treer Issues in Pockymoe', assignee: threads.assignee.agent, labels: ['lab'] });
    issue = created.issue;
    assert.equal(issue.creator, threads.author.agent);
    assert.equal(created.notification.status, 'sent');
    await until('notification stored in inbox', async () =>
      (await sql('SELECT count(*) FROM treer_message_inbound WHERE thread_id=?', threads.assignee.thread))[0][0] > 0, 60_000);
    await new Promise(resolve => setTimeout(resolve, 3000));
    assert.equal(await turns(threads.assignee), before, 'Agent mail stays passive');
  });

  await check('Issues reminder from the App identity wakes the idle assignee', async () => {
    const before = await turns(threads.assignee);
    // ISSUES_REMINDER_FIRST=1m plus one 60 s reminder poll and our 2 s receive loop.
    await until('reminder turn', async () => {
      const rows = await sql(`SELECT count(*) FROM thread_history_items WHERE thread_id=? AND item_json LIKE '%Issues reminder%' AND item_json LIKE '%http://issues.treer/%'`, threads.assignee.thread);
      return rows[0][0] > 0 && await turns(threads.assignee) > before;
    }, 240_000);
  });

  await check('Assignee comments and resolves the issue under its own identity', async () => {
    await issues(threads.assignee, 'POST', `/v1/issues/${issue.id}/comment`, { body: `@${threads.author.name} done in lab` });
    const done = await issues(threads.assignee, 'PATCH', `/v1/issues/${issue.id}`, { status: 'done' });
    assert.equal(done.issue.status, 'done');
    const shown = await issues(threads.author, 'GET', `/v1/issues/${issue.id}`);
    const actors = shown.activity.filter(e => e.kind !== 'create').map(e => e.actor);
    assert.ok(actors.length >= 2 && actors.every(a => a === threads.assignee.agent));
    await until('creator notified', async () =>
      (await sql('SELECT count(*) FROM treer_message_inbound WHERE thread_id=?', threads.author.thread))[0][0] > 0, 60_000);
  });

  await check('Deleting the test Agents archives their threads', async () => {
    for (const t of Object.values(threads)) await managed(['agent', 'admin', 'delete', t.agent]);
    // The registry reconciles each minute.
    await until('threads archived', async () => {
      for (const t of Object.values(threads)) {
        const rows = await sql('SELECT state FROM treer_registry WHERE thread_id=?', t.thread);
        if (rows[0]?.[0] !== 'archived') return false;
      }
      return true;
    }, 150_000);
  });
} finally {
  await mkdir(evidenceDir, { recursive: true, mode: 0o700 });
  await writeFile(`${evidenceDir}/evidence.json`, JSON.stringify({ at: new Date().toISOString(), checks }, null, 2) + '\n', { mode: 0o600 });
}
