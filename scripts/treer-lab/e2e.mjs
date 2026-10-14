import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { root, treerSource, read, save, api, until, deviceExec, requireDocker, run } from './common.mjs';

const checks = [];
async function check(name, fn) {
  try { await fn(); }
  catch (error) {
    checks.push({ name, result: 'failed', failure: error.message });
    throw error;
  }
  checks.push({ name, result: 'passed' });
  console.log(`PASS ${name}`);
}
// Jobs contain CLI argv only. Treer identity is supplied by Host to the worker Agent.
async function managed(args) {
  const id = randomUUID();
  await deviceExec(['/bin/sh', '-c', 'umask 077; cat > "$1"', 'sh', `/workspace/lab-jobs/${id}.request.json`],
    { input: JSON.stringify(args) });
  const response = await until('managed CLI job', async () => {
    try { return JSON.parse(await deviceExec(['cat', `/workspace/lab-jobs/${id}.reply.json`])); }
    catch { return false; }
  }, 70_000);
  assert.equal(response.code, 0, `Managed treer ${args.slice(0, 3).join(' ')} failed`);
  assert.notEqual(response.value, null);
  return response.value;
}

try {
  await requireDocker();
  const state = await read();
  const ws = `/api/workspaces/${state.workspaceId}`;
  const snapshot = () => api(`${ws}/snapshot`);
  let machine, worker;
  await check('Proxy and authenticated lab admin/user', async () => {
    assert.equal(state.authEnabled, true);
    assert.ok(await api('/api/health'));
    assert.ok(await api('/api/admin/me'));
    assert.ok(await api('/api/auth/me'));
  });
  if (process.argv.includes('--full')) {
    try { await deviceExec(['pockymoe', 'attach', '--help']); }
    catch { throw new Error('Full E2E requires the integrated H/A binary: pockymoe attach is unavailable'); }
  }
  await check('Docker host device enrolled and online', async () => {
    machine = await until('enrolled host online', async () =>
      (await snapshot()).servers.find(server => server.status === 'online' && server.name === 'lab-device'));
    assert.ok(machine);
    assert.equal(machine.root, '/workspace');
  });
  await check('Plain terminal Agent accepts input and returns output', async () => {
    const name = `lab-terminal-${randomUUID().slice(0, 8)}`;
    await api(`${ws}/agents`, { server_id: machine.server_id, name, kind: 'command', cwd: 'project',
      args: ['/bin/bash', '--noprofile', '--norc'] });
    const agent = await until('terminal agent', async () => (await snapshot()).agents.find(a => a.name === name));
    const marker = `LAB_TERMINAL_${randomUUID().replaceAll('-', '')}`;
    // A computed value proves shell execution; terminal echo alone cannot pass.
    await api(`${ws}/agents/${agent.agent_id}/input`, { data: [...Buffer.from(`printf 'LAB_%s\\n' '${marker.slice(4)}'\n`)] });
    await until('terminal command output', async () => {
      const output = await api(`${ws}/agents/${agent.agent_id}/output?lines=80`);
      return JSON.stringify(output).includes(marker);
    });
    await api(`${ws}/agents/${agent.agent_id}/stop`, {});
  });
  await check('Managed Agent caller and Controller contract', async () => {
    // A prior failed driver may have left a worker consuming the same job directory.
    for (const old of (await snapshot()).agents.filter(a => a.name.startsWith('lab-driver-') && a.status !== 'exited')) {
      await api(`${ws}/agents/${old.agent_id}/stop`, {});
    }
    await deviceExec(['rm', '-f', '/workspace/lab-jobs/ready.json']);
    const name = `lab-driver-${randomUUID().slice(0, 8)}`;
    await api(`${ws}/agents`, { server_id: machine.server_id, name, kind: 'command', cwd: '.',
      args: ['/usr/local/bin/node', '/opt/lab/managed-driver.mjs'] });
    worker = await until('managed driver identity', async () => {
      try { return JSON.parse(await deviceExec(['cat', '/workspace/lab-jobs/ready.json'])); } catch { return false; }
    });
    const identity = await managed(['whoami']);
    assert.equal(identity.agent.agent_id, worker.agentId);
    const contract = await managed(['interface', 'contract']);
    assert.equal(contract.protocol, 'treer.agent-interface/v1');
  });
  await check('Fake Pockymoe hub is a Treer Managed App created by a managed Agent', async () => {
    const existing = (await api(`${ws}/apps`)).apps?.find(app => app.name === 'pockymoe-lab-hub');
    if (!existing) {
      await managed(['app', 'create', '--machine', machine.server_id, '--name', 'pockymoe-lab-hub',
        '--cwd', '.', '--port', '18880', '/opt/lab/hub.sh']);
    }
    await until('fake hub health', async () => {
      const health = JSON.parse(await deviceExec(['curl', '-fsS', 'http://127.0.0.1:18880/healthz']));
      return health;
    });
    if (process.argv.includes('--full')) {
      assert.equal((await deviceExec(['stat', '-c', '%a', '/workspace/hub/treer-attach.token'])).trim(), '600');
      assert.equal((await deviceExec(['sh', '-c', 'command -v treer'])).trim(), '/usr/local/bin/treer');
    }
  });
  if (!process.argv.includes('--full')) {
    await api(`${ws}/agents/${worker.agentId}/stop`, {});
    await save('evidence.json', { mode: 'baseline', build: await read('build.json'), checks,
      deferred: ['hub/attach AIS', 'prompt/transcript/status', 'UI tunnel and cross-thread gate', 'stop/start/reconnect'],
      recordedAt: new Date().toISOString() });
    console.log('Baseline complete. --full requires the merged H/A implementation.');
  } else {
    const attachArgs = JSON.parse(process.env.TREER_LAB_ATTACH_ARGS_JSON ??
      '["attach","--provider","codex","--hub","http://127.0.0.1:18880","--hub-token-file","/workspace/hub/treer-attach.token"]');
    assert.ok(Array.isArray(attachArgs) && attachArgs.every(a => typeof a === 'string'));
    const profile = `lab-attach-${randomUUID().slice(0, 8)}`;
    await managed(['agent', 'admin', 'profile', 'create', profile, '--cwd', 'project',
      '/usr/local/bin/pockymoe', '--', ...attachArgs]);
    async function launch(name) {
      await managed(['agent', 'admin', 'profile', 'launch', profile, '--machine', machine.server_id, '--name', name]);
      return until('AIS descriptor', async () => {
        const agent = (await snapshot()).agents.find(a => a.name === name);
        return agent?.interface && agent;
      });
    }
    let first, second;
    await check('Two attach Agents register distinct interfaces on the hub port', async () => {
      first = await launch(`lab-first-${randomUUID().slice(0, 8)}`);
      second = await launch(`lab-second-${randomUUID().slice(0, 8)}`);
      assert.equal(first.interface.protocol, 'treer.agent-interface/v1');
      assert.equal(first.interface.port, second.interface.port);
      assert.notEqual(first.interface.instance_id, second.interface.instance_id);
      for (const capability of ['prompt.submit', 'prompt.queue', 'prompt.queue.read', 'prompt.notify',
        'transcript.read', 'state.observe', 'abort']) {
        assert.ok(first.interface.capabilities.includes(capability), `Missing required ${capability}`);
      }
    });
    const scopedThreads = async agentId => {
      const result = await api(`${ws}/agents/${agentId}/interface/ui/api/threads`);
      return Array.isArray(result) ? result : result.threads;
    };
    const ownThreads = await scopedThreads(first.agent_id);
    assert.equal(ownThreads.length, 1, 'Fresh Agent should expose exactly its bound thread');
    const threadId = ownThreads[0].id;
    const hubDetail = async () => JSON.parse(await deviceExec(['curl', '-fsS',
      `http://127.0.0.1:18880/api/threads/${threadId}`]));
    const loadedThreads = async () => JSON.parse(await deviceExec(['curl', '-fsS',
      'http://127.0.0.1:18880/api/device/processes'])).threads;
    let originalHistory;
    const promptIds = [];
    await check('CLI prompt produces replies, idle status and two transcript pages', async () => {
      for (const text of ['Treer lab first turn', 'Treer lab second turn']) {
        const receipt = await managed(['agent', 'prompt', first.agent_id, text, '--notify']);
        assert.ok(receipt.submission?.id);
        promptIds.push(receipt.submission.id);
        await until('completed fake turn', async () => {
          const transcript = await managed(['agent', 'transcript', first.agent_id, '--page', text.includes('first') ? '0' : '1', '--limit', '1']);
          return transcript.entries?.some(entry => entry.role === 'assistant' && entry.content === `ok: ${text}`)
            && transcript.entries.some(entry => entry.role === 'user' && entry.content === text) && transcript;
        });
        await until('idle after reply', async () => {
          const show = await managed(['agent', 'show', first.agent_id]);
          return show.status === 'idle' && show.busy === false;
        });
      }
      originalHistory = await managed(['agent', 'transcript', first.agent_id, '--page', '0', '--limit', '1']);
      assert.ok(originalHistory.page_count >= 2);
      const cursorHistory = await managed(['agent', 'transcript', first.agent_id, '--cursor', '0', '--limit', '1']);
      assert.deepEqual(cursorHistory.entries, originalHistory.entries);
      const show = await managed(['agent', 'show', first.agent_id]);
      assert.equal((show.agent ?? show).status, 'idle');
    });
    await check('Agent-to-Agent queued prompt sends a completion notification', async () => {
      await until('completion Core Message', async () => {
        const messages = await managed(['message', 'receive']);
        // Treer completion bodies include submission IDs and a bounded assistant preview.
        const body = JSON.stringify(messages);
        return promptIds.some(id => body.includes(id)) && body.includes('done') && messages;
      });
      await managed(['agent', 'queue', 'ls', first.agent_id]);
    });
    // This config starts no local Supervisor; it targets only the lab's Treer tunnel.
    await save('browser.json', { workspaceId: state.workspaceId, firstAgentId: first.agent_id,
      secondAgentId: second.agent_id });
    await check('Chromium thread UI and cross-thread API gate', async () => {
      await run(process.execPath, [`${treerSource}/web/node_modules/@playwright/test/cli.js`, 'test',
        'tunnel.spec.mjs', '--config', `${root}/scripts/treer-lab/playwright.config.mjs`, '--project=desktop-chromium'],
      { inherit: true, timeout: 120_000 });
    });
    await check('Stop/start keeps the same Agent, bound thread and history', async () => {
      // This fake-runtime trigger delays the turn for 25 seconds; stop must interrupt it.
      await managed(['agent', 'prompt', first.agent_id, 'inspect this repository for the Treer lab stop check', '--steer']);
      await until('working and busy', async () => {
        const show = await managed(['agent', 'show', first.agent_id]);
        return show.status === 'working' && show.busy === true;
      });
      assert.ok((await loadedThreads()).some(thread => thread.threadId === threadId), 'Stop fixture must have a loaded harness');
      await managed(['agent', 'stop', first.agent_id]);
      await until('Agent stopped', async () => (await snapshot()).agents.find(a => a.agent_id === first.agent_id)?.status === 'exited');
      await until('hub interrupted turn and released harness', async () => {
        const detail = await hubDetail();
        return detail.thread.activeTurnId == null && detail.turns.some(turn => turn.status === 'interrupted')
          && !(await loadedThreads()).some(thread => thread.threadId === threadId);
      });
      await managed(['agent', 'start', first.agent_id]);
      await until('rebound interface', async () => (await snapshot()).agents.find(a => a.agent_id === first.agent_id)?.interface);
      assert.equal((await scopedThreads(first.agent_id))[0].id, threadId);
      const history = await managed(['agent', 'transcript', first.agent_id, '--page', '0', '--limit', '1']);
      assert.deepEqual(history.entries, originalHistory.entries);
      assert.equal((await scopedThreads(first.agent_id))[0].id, threadId);
    });
    await check('Hub App restart reconnects existing attach processes', async () => {
      const pid = (await snapshot()).agents.find(a => a.agent_id === first.agent_id).pid;
      const before = await managed(['app', 'show', '/pockymoe-lab-hub']);
      await managed(['app', 'restart', '/pockymoe-lab-hub']);
      await until('new managed hub process', async () => {
        const after = await managed(['app', 'show', '/pockymoe-lab-hub']);
        const oldApp = before.app ?? before.deployment ?? before;
        const newApp = after.app ?? after.deployment ?? after;
        return newApp.pid && newApp.pid !== oldApp.pid;
      });
      await until('AIS after hub restart', async () => {
        const history = await managed(['agent', 'transcript', first.agent_id, '--page', '0', '--limit', '1']);
        return history.entries?.length && history;
      });
      assert.equal((await snapshot()).agents.find(a => a.agent_id === first.agent_id).pid, pid);
      const history = await managed(['agent', 'transcript', first.agent_id, '--page', '0', '--limit', '1']);
      assert.deepEqual(history.entries, originalHistory.entries);
    });
    await save('full-evidence.json', { mode: 'full', build: await read('build.json'), checks, recordedAt: new Date().toISOString() });
    await api(`${ws}/agents/${worker.agentId}/stop`, {});
  }
} catch (error) {
  await save(process.argv.includes('--full') ? 'full-evidence.json' : 'evidence.json',
    { checks, failure: error.message, recordedAt: new Date().toISOString() });
  console.error(error.message); process.exitCode = 1;
}
