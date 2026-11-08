import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:net';
import { createServer as createHttpServer, request as httpRequest } from 'node:http';
import { once } from 'node:events';
import { mkdtempSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { apply } from '../crates/runtime/src/acp/deepseek-bridge.mjs';

// A minimal DSH context: event listeners, injected services and appReady.
function fakeDsh(overrides = {}) {
  const listeners = new Map();
  let ready;
  const agent = { id: 'session-1', session: { id: 'session-1' } };
  const calls = [];
  const services = {
    agents: {
      roots: () => [agent],
      list: () => [agent],
      isOwnedBy: () => false,
      get: id => (id === agent.id ? agent : undefined),
    },
    sessionProjections: {
      onChanged: () => () => {},
      snapshot: () => ({ values: { plan: { active: false, pending: false }, permissions: { currentValue: 'workspace-write' } } }),
    },
    typertGateway: {
      invoke: async ({ namespace, method, args }) => {
        calls.push(`${namespace}/${method}`);
        if (namespace === 'pluginManager' && method === 'listPlugins') {
          return [
            { entryId: 'include', moduleName: 'cordis:include', enabled: true, readOnlyReason: 'unaddressable' },
            { entryId: 'include:llm', moduleName: '@deepseek-ai/dsh-llm', enabled: true, fiberPhase: 'active',
              meta: { title: 'LLM', description: 'Provider-neutral' }, patchId: 'llm', config: { apiKey: 'never-export-this' } },
          ];
        }
        if (namespace === 'permissionPresets') return { options: [{ value: 'read-only' }, { value: 'workspace-write' }], defaultPreset: 'workspace-write' };
        if (namespace === 'commands' && method === 'list') {
          return [{ name: 'plan', description: 'Plan' }, { name: 'review', description: 'Review', input: { hint: '<path>' } }];
        }
        if (namespace === 'commands' && method === 'execute') return { commandId: 'c1', result: { kind: 'success', text: args.line } };
        return [];
      },
    },
    commands: {},
    goals: {},
    ...overrides,
  };
  const ctx = {
    appReady: { onReady: fn => { ready = fn; return () => {}; } },
    on: (event, fn) => { listeners.set(event, fn); },
    inject: (deps, fn) => fn(ctx),
    get: name => services[name],
    loader: { entries: () => [] },
    llm: {
      listProviders: () => [{ id: 'custom', name: 'Custom' }],
      listModels: async () => [{ id: 'with-reasoning', name: 'Reasoning' }, { id: 'plain', name: 'Plain' }],
      resolveModelInfo: async (_, model) => (model === 'plain' ? {} : { reasoning: { efforts: [{ id: 'xhigh', name: 'Xhigh' }] } }),
    },
    ...services,
  };
  return { ctx, listeners, agent, calls, ready: () => ready() };
}

async function supervisor() {
  const server = createServer();
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const connection = new Promise(resolve => server.once('connection', resolve));
  return { server, port: server.address().port, connection };
}

// The plugin's dispose effect closes its socket; then the server can close.
async function stop(server, dsh) {
  dsh?.listeners.get('dispose')?.();
  await new Promise(resolve => server.close(resolve));
}

function lines(socket) {
  const queue = [];
  const waiters = [];
  let buffer = '';
  socket.on('data', data => {
    buffer += data;
    let index;
    while ((index = buffer.indexOf('\n')) >= 0) {
      const message = JSON.parse(buffer.slice(0, index));
      buffer = buffer.slice(index + 1);
      const waiter = waiters.shift();
      if (waiter) waiter(message); else queue.push(message);
    }
  });
  return () => (queue.length ? Promise.resolve(queue.shift()) : new Promise(resolve => waiters.push(resolve)));
}

test('steering injects into the running loop without waking idle sessions or following up', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh({ agentLoop: {} });
  const received = [];
  dsh.agent.inject = message => received.push(message);
  dsh.agent.followup = () => assert.fail('steering must not queue a new turn');
  dsh.agent.steer = () => assert.fail('steering must not wake an idle loop');
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    assert.equal((await next()).hello.features.steer, true);
    const send = (id, sessionId, prompt) => socket.write(JSON.stringify({ id, method: 'steer', params: { sessionId, prompt } }) + '\n');
    dsh.agent.status = 'running';
    send(1, 'session-1', 'Change direction now');
    assert.equal((await next()).result.outcome, 'injected');
    assert.equal(received.length, 1);
    assert.equal(received[0].role, 'user');
    assert.deepEqual(received[0].source, { kind: 'user' });
    assert.deepEqual(received[0].content, [{ type: 'text', text: 'Change direction now' }]);
    assert.match(received[0].id, /^[0-9a-f-]{36}$/);
    dsh.agent.status = 'idle';
    send(2, 'session-1', 'Too late');
    assert.deepEqual((await next()).result, { outcome: 'promptRequired', reason: 'noRunningTurn' });
    assert.equal(received.length, 1);
    send(3, 'child-session', 'Wrong owner');
    assert.match((await next()).error.message, /not live/);
    dsh.agent.status = 'running';
    send(4, 'session-1', '  ');
    assert.match((await next()).error.message, /text prompt/);
    delete dsh.agent.inject;
    send(5, 'session-1', 'Unsupported');
    assert.match((await next()).error.message, /does not support steering/);
  } finally { await stop(server, dsh); }
});

test('hello carries per-model reasoning and allowlisted inventory, never plugin config', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  try {
    apply(dsh.ctx, { port, token: 'test-token' });
    dsh.ready();
    const next = lines(await connection);
    const hello = await next();
    assert.equal(hello.token, 'test-token');
    assert.equal(hello.hello.protocol, 1);
    assert.equal(hello.hello.features.steer, false);
    assert.equal(hello.hello.models[0].model, '["custom","with-reasoning"]');
    assert.deepEqual(hello.hello.models[0].supportedReasoningEfforts.map(e => e.reasoningEffort), ['', 'xhigh']);
    assert.deepEqual(hello.hello.models[1].supportedReasoningEfforts, []);
    assert.deepEqual(hello.hello.plugins.map(plugin => plugin.id), ['include:llm']);
    assert.deepEqual(hello.hello.permissionPresets, ['read-only', 'workspace-write']);
    assert.equal(hello.hello.features.commands, true);
    assert.equal(JSON.stringify(hello).includes('never-export-this'), false);
  } finally { await stop(server, dsh); }
});

test('remote calls and commands are allowlisted; settings accept scalars only', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh({ settings: { describe: () => [], mutate: async () => {} } });
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const send = message => socket.write(JSON.stringify(message) + '\n');
    send({ id: 1, method: 'invoke', params: { namespace: 'credentials', method: 'set', args: {} } });
    assert.match((await next()).error.message, /not available/);
    // Any command DSH registered for the session runs, plugin commands included.
    send({ id: 2, method: 'command', params: { sessionId: 'session-1', line: '/unknown leak' } });
    assert.match((await next()).error.message, /has no command \/unknown/);
    send({ id: 9, method: 'command', params: { sessionId: 'session-1', line: 'review src' } });
    assert.match((await next()).error.message, /has no command/);
    send({ id: 10, method: 'command', params: { sessionId: 'session-1', line: '/review src' } });
    assert.equal((await next()).result.result.text, '/review src');
    send({ id: 3, method: 'command', params: { sessionId: 'session-1', line: '/plan off' } });
    assert.equal((await next()).result.result.text, '/plan off');
    send({ id: 4, method: 'updateSetting', params: { ns: 'llm-pi-ai', key: 'providers', value: 'x' } });
    assert.match((await next()).error.message, /not editable/);
    send({ id: 5, method: 'updateSetting', params: { ns: 'permission', key: 'defaultPreset', value: '!!js 1', revision: 1 } });
    assert.match((await next()).error.message, /YAML tags/);
    send({ id: 7, method: 'updateSetting', params: { ns: 'agent-loop', key: 'maxParallelToolCalls', value: '4', revision: 1 } });
    assert.match((await next()).error.message, /expects a number/);
    send({ id: 8, method: 'updateSetting', params: { ns: 'agent-loop', key: 'maxParallelToolCalls', value: 4 } });
    assert.match((await next()).error.message, /revision is required/);
    send({ id: 6, method: 'session', params: { sessionId: 'session-1' } });
    const session = (await next()).result;
    assert.deepEqual(session.commands, [
      { name: 'plan', description: 'Plan', hint: null },
      { name: 'review', description: 'Review', hint: '<path>' },
    ]);
    assert.equal(session.projections.permissions.currentValue, 'workspace-write');
  } finally { await stop(server, dsh); }
});

test('questions wait for the supervisor and coalesced stream text reaches it', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const asked = dsh.listeners.get('user-questions/request')(
      { agent: dsh.agent, questions: [{ id: 'q', question: 'Approve?', options: [{ label: 'Yes' }] }] },
      () => Promise.reject(new Error('fell through')),
    );
    const question = await next();
    assert.equal(question.event, 'question');
    assert.equal(question.sessionId, 'session-1');
    socket.write(JSON.stringify({ answer: question.questionId, result: { answers: [{ id: 'q', selected: ['Yes'] }] } }) + '\n');
    assert.deepEqual(await asked, { answers: [{ id: 'q', selected: ['Yes'] }] });

    const stream = dsh.listeners.get('agent/assistant-stream');
    stream({ agent: dsh.agent, frame: { type: 'start', attemptId: 'a1', step: 1 } });
    for (const text of ['Hel', 'lo']) {
      stream({ agent: dsh.agent, frame: { type: 'chunk', attemptId: 'a1', chunk: { type: 'text-delta', text } } });
    }
    stream({ agent: dsh.agent, frame: { type: 'end', attemptId: 'a1', outcome: { kind: 'committed', eventType: 'assistant/message' } } });
    assert.equal((await next()).kind, 'start');
    assert.deepEqual(await next(), { event: 'stream', sessionId: 'session-1', attemptId: 'a1', kind: 'text', text: 'Hello' });
    assert.equal((await next()).outcome, 'committed');
  } finally { await stop(server, dsh); }
});

test('profile edits are backed up first and application bundles stay locked', async () => {
  const { server, port, connection } = await supervisor();
  const dir = mkdtempSync(join(tmpdir(), 'dsh-profile-'));
  writeFileSync(join(dir, 'cordis.patch.yml'), '[ { id: web, disabled: true } ]\n');
  writeFileSync(join(dir, 'package.json'), '{}');
  const dsh = fakeDsh({ profileContext: { name: 'acp', dir, patchPath: join(dir, 'cordis.patch.yml'), installAnchor: join(dir, 'package.json') } });
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const send = message => socket.write(JSON.stringify(message) + '\n');
    send({ id: 1, method: 'invoke', params: { namespace: 'pluginManager', method: 'setBundleEnabled', args: { name: '@deepseek-ai/dsh-headless', enabled: true } } });
    assert.match((await next()).error.message, /application bundle/);
    assert.throws(() => readdirSync(join(dir, '.remote-codex', 'backups')));
    send({ id: 2, method: 'invoke', params: { namespace: 'pluginManager', method: 'setPluginEnabled', args: { id: 'include:web', enabled: true } } });
    await next();
    const [backup] = readdirSync(join(dir, '.remote-codex', 'backups'));
    assert.equal(readFileSync(join(dir, '.remote-codex', 'backups', backup, 'cordis.patch.yml'), 'utf8'), '[ { id: web, disabled: true } ]\n');
  } finally { await stop(server, dsh); }
});

test('background subagents keep their root session busy', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  const child = { id: 'child-1', status: 'idle' };
  dsh.agent.status = 'idle';
  dsh.ctx.agents.list = () => [dsh.agent, child];
  dsh.ctx.agents.isOwnedBy = (id, owner) => id === 'child-1' && owner === dsh.agent;
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const next = lines(await connection);
    await next();
    const status = dsh.listeners.get('agent/status');
    status({ agent: dsh.agent, status: 'running' });
    assert.deepEqual(await next(), { event: 'status', sessionId: 'session-1', status: 'running' });
    // The root goes idle while its background child still runs: still busy.
    child.status = 'running';
    status({ agent: dsh.agent, status: 'idle' });
    status({ agent: child, status: 'idle' });
    assert.deepEqual(await next(), { event: 'status', sessionId: 'session-1', status: 'idle' });
  } finally { await stop(server, dsh); }
});

test('replies keep multibyte text split across reads', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const asked = dsh.listeners.get('user-questions/request')(
      { agent: dsh.agent, questions: [{ id: 'q', question: '继续?' }] },
      () => Promise.reject(new Error('fell through')),
    );
    const question = await next();
    const reply = Buffer.from(JSON.stringify({ answer: question.questionId, result: { answers: [{ id: 'q', selected: [], custom: '先写测试' }] } }) + '\n');
    const cut = reply.indexOf(Buffer.from('写')) + 1;
    socket.write(reply.subarray(0, cut));
    await new Promise(resolve => setTimeout(resolve, 20));
    socket.write(reply.subarray(cut));
    assert.equal((await asked).answers[0].custom, '先写测试');
  } finally { await stop(server, dsh); }
});

// Agent presets as the native Web composition registers them.
function presets(selected) {
  return {
    defaultId: 'standard',
    list: async () => [{ id: 'standard' }, { id: 'minimal', name: 'Minimal' }, { id: 'draft', broken: 'missing plugin' }],
    select: async (agent, id) => { selected.push(['select', agent.id, id]); return id; },
    recompose: async (_ctx, id) => { selected.push(['recompose', id]); },
  };
}

test('run modes bind before the first turn and are fixed after it', async () => {
  const { server, port, connection } = await supervisor();
  const selected = [];
  let recorded = null;
  let boundary;
  const dsh = fakeDsh({
    agentPresets: presets(selected),
    sessionProjections: {
      onChanged: () => () => {},
      snapshot: () => ({ values: { agentPreset: recorded } }),
      stateOf: () => boundary,
    },
  });
  try {
    apply(dsh.ctx, { port, token: 't', preset: 'minimal' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    const { hello } = await next();
    assert.deepEqual(hello.runModes.map(({ id, isDefault, broken }) => [id, isDefault, broken]),
      [['standard', true, null], ['minimal', false, null], ['draft', false, 'missing plugin']]);
    assert.equal(hello.features.runModes, true);
    const created = dsh.listeners.get('agent/created');
    // A new session takes the launch preset; a recorded one keeps its own.
    await created({ agent: dsh.agent });
    recorded = 'standard';
    await created({ agent: dsh.agent });
    // A session from before run modes keeps its log and gets the default tools.
    recorded = null;
    boundary = { openTurnStartSeq: null, lastTurn: 2 };
    await created({ agent: dsh.agent });
    // Subagents keep whatever their parent gave them.
    await created({ agent: { id: 'child' } });
    assert.deepEqual(selected, [['select', 'session-1', 'minimal'], ['recompose', 'standard'], ['recompose', 'standard']]);

    const send = message => socket.write(JSON.stringify(message) + '\n');
    boundary = undefined;
    send({ id: 1, method: 'session', params: { sessionId: 'session-1' } });
    assert.equal((await next()).result.presetLocked, false);
    send({ id: 2, method: 'selectPreset', params: { sessionId: 'session-1', preset: 'minimal' } });
    assert.deepEqual((await next()).result, { agentPreset: 'minimal' });
    boundary = { openTurnStartSeq: 7, lastTurn: 0 };
    send({ id: 3, method: 'session', params: { sessionId: 'session-1' } });
    assert.equal((await next()).result.presetLocked, true);
    send({ id: 4, method: 'selectPreset', params: { sessionId: 'nobody', preset: 'minimal' } });
    assert.match((await next()).error.message, /not live/);
  } finally { await stop(server, dsh); }
});

function get(port, headers, path = '/') {
  return new Promise((resolve, reject) => {
    const req = httpRequest({ host: '127.0.0.1', port, path, headers }, res => {
      let body = '';
      res.setEncoding('utf8');
      res.on('data', chunk => { body += chunk; });
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, body }));
    });
    req.on('error', reject);
    req.end();
  });
}

test('the native console proxy forwards loopback pages only, with DSH as the authority', async () => {
  const { server, port, connection } = await supervisor();
  const seen = [];
  const web = createHttpServer((req, res) => {
    seen.push({ host: req.headers.host, origin: req.headers.origin ?? null, url: req.url });
    res.writeHead(303, { location: './', 'set-cookie': 'dsh=1; Path=/; HttpOnly; SameSite=Strict' }).end();
  });
  web.listen(0, '127.0.0.1');
  await once(web, 'listening');
  const webPort = web.address().port;
  const dsh = fakeDsh({
    webServer: { port: webPort },
    connection: { authenticatedUrl: base => `${base}?token=launch-token` },
  });
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    assert.equal((await next()).hello.features.console, true);
    socket.write(JSON.stringify({ id: 1, method: 'console', params: {} }) + '\n');
    const { port: proxy, path } = (await next()).result;
    assert.equal(path, '/?token=launch-token');
    const self = `127.0.0.1:${proxy}`;
    // Same-origin page (loopback or a Relay preview, which presents loopback).
    const ok = await get(proxy, { host: self, origin: `http://${self}` }, path);
    // The token login keeps DSH's cookie but continues from the page itself,
    // so a cross-site opener cannot make the browser drop it.
    assert.equal(ok.status, 200);
    assert.equal(ok.headers.location, undefined);
    assert.match(String(ok.headers['set-cookie']), /dsh=1; Path=\/; HttpOnly; SameSite=Strict/);
    assert.equal(ok.headers['cache-control'], 'no-store');
    assert.match(ok.body, /http-equiv="refresh" content="0;url=\.\/"/);
    assert.deepEqual(seen.at(-1), { host: `127.0.0.1:${webPort}`, origin: `http://127.0.0.1:${webPort}`, url: path });
    // Other redirects pass through untouched.
    assert.equal((await get(proxy, { host: self }, '/elsewhere')).status, 303);
    // A cross-origin page keeps its Origin, so DSH's own fence refuses it.
    await get(proxy, { host: self, origin: 'http://evil.example' });
    assert.equal(seen.at(-1).origin, 'http://evil.example');
    // DNS-rebound or public names never reach DSH.
    const count = seen.length;
    for (const host of ['evil.example', `p-${'a'.repeat(32)}.example.com`, `127.0.0.1.evil.example:${proxy}`]) {
      assert.equal((await get(proxy, { host })).status, 403);
    }
    assert.equal(seen.length, count);
    // Reopening reuses the proxy.
    socket.write(JSON.stringify({ id: 2, method: 'console', params: {} }) + '\n');
    assert.equal((await next()).result.port, proxy);
  } finally {
    await stop(server, dsh);
    await new Promise(resolve => web.close(resolve));
  }
});

test('compositions without a Web host report no console or run modes', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    const { hello } = await next();
    assert.equal(hello.features.console, false);
    assert.equal(hello.features.runModes, false);
    assert.deepEqual(hello.runModes, []);
    socket.write(JSON.stringify({ id: 1, method: 'console', params: {} }) + '\n');
    assert.match((await next()).error.message, /no native console/);
    socket.write(JSON.stringify({ id: 2, method: 'selectPreset', params: { sessionId: 'session-1', preset: 'minimal' } }) + '\n');
    assert.match((await next()).error.message, /no run modes/);
  } finally { await stop(server, dsh); }
});

test('thread titles replace the prompt-derived DSH title once', async () => {
  const { server, port, connection } = await supervisor();
  const titles = new Map();
  const renamed = [];
  const dsh = fakeDsh({
    sessionTitle: {
      get: session => titles.get(session.id),
      rename: (session, title) => {
        renamed.push(title);
        titles.set(session.id, { title });
        return { title };
      },
    },
  });
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const send = message => socket.write(JSON.stringify(message) + '\n');
    send({ id: 1, method: 'rename', params: { sessionId: 'session-1', title: ' Fix the parser ' } });
    assert.deepEqual((await next()).result, { title: 'Fix the parser' });
    send({ id: 2, method: 'rename', params: { sessionId: 'session-1', title: 'Fix the parser' } });
    await next();
    assert.deepEqual(renamed, ['Fix the parser']);
    send({ id: 3, method: 'rename', params: { sessionId: 'gone', title: 'x' } });
    assert.match((await next()).error.message, /not live/);
  } finally { await stop(server, dsh); }
});

test('the console proxy closes DSH streams with the browser and on dispose', async () => {
  const { server, port, connection } = await supervisor();
  let open = 0;
  const web = createHttpServer((req, res) => {
    open += 1;
    req.socket.once('close', () => { open -= 1; });
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.write('data: hello\n\n');
  });
  web.listen(0, '127.0.0.1');
  await once(web, 'listening');
  const dsh = fakeDsh({
    webServer: { port: web.address().port },
    connection: { authenticatedUrl: base => `${base}?token=t` },
  });
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    // Two panels asking at once share one proxy.
    socket.write(JSON.stringify({ id: 1, method: 'console', params: {} }) + '\n');
    socket.write(JSON.stringify({ id: 2, method: 'console', params: {} }) + '\n');
    const [a, b] = [await next(), await next()];
    assert.equal(a.result.port, b.result.port);
    const proxy = a.result.port;
    const stream = () => new Promise(resolve => {
      const req = httpRequest({ host: '127.0.0.1', port: proxy, path: '/plugins/events', headers: { host: `127.0.0.1:${proxy}` } },
        res => res.once('data', () => resolve(req)));
      req.end();
    });
    const first = await stream();
    await stream();
    assert.equal(open, 2);
    first.destroy();
    await new Promise(resolve => setTimeout(resolve, 100));
    assert.equal(open, 1, 'a closed browser stream must close its DSH stream');
    dsh.listeners.get('dispose')();
    await new Promise(resolve => setTimeout(resolve, 100));
    assert.equal(open, 0, 'dispose must close every proxied stream');
  } finally {
    await new Promise(resolve => server.close(resolve));
    web.closeAllConnections();
    await new Promise(resolve => web.close(resolve));
  }
});

test('model-call usage is reported for the root session, subagents included', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  const child = { id: 'child-1', session: { id: 'child-session' } };
  dsh.ctx.agents.list = () => [dsh.agent, child];
  dsh.ctx.agents.isOwnedBy = (id, owner) => id === 'child-1' && owner === dsh.agent;
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const next = lines(await connection);
    await next();
    const onEvent = dsh.listeners.get('session/event');
    const usage = { inputTokens: 10, outputTokens: 5, cacheReadTokens: 90 };
    onEvent(child.session, { type: 'assistant/message', seq: 4, data: { usage } });
    assert.deepEqual(await next(), { event: 'usage', sessionId: 'session-1', reportId: 'child-session:4', usage });
    // Other events and messages without usage report nothing.
    onEvent(dsh.agent.session, { type: 'tool/result', seq: 5, data: {} });
    onEvent(dsh.agent.session, { type: 'assistant/message', seq: 6, data: {} });
    onEvent(dsh.agent.session, { type: 'assistant/message', seq: 7, data: { usage } });
    assert.equal((await next()).reportId, 'session-1:7');
  } finally { await stop(server, dsh); }
});

test('tool approvals reach Pockymoe for the root session and fail closed', async () => {
  const { server, port, connection } = await supervisor();
  const dsh = fakeDsh();
  const child = { id: 'child-1', session: { id: 'child-session' } };
  dsh.ctx.agents.list = () => [dsh.agent, child];
  dsh.ctx.agents.isOwnedBy = (id, owner) => id === 'child-1' && owner === dsh.agent;
  try {
    apply(dsh.ctx, { port, token: 't' });
    dsh.ready();
    const socket = await connection;
    const next = lines(socket);
    await next();
    const approve = dsh.listeners.get('approval/request');
    const fallthrough = () => Promise.resolve('unavailable');
    // A subagent's approval belongs to its root session's turn.
    const allowed = approve({ agent: child, toolName: 'bash', callId: 'c1', reason: 'write the report' }, fallthrough);
    const asked = await next();
    assert.equal(asked.event, 'approval');
    assert.equal(asked.sessionId, 'session-1');
    assert.deepEqual(asked.request, { toolName: 'bash', callId: 'c1', reason: 'write the report', subagent: true });
    socket.write(JSON.stringify({ answer: asked.questionId, result: 'allowed-once' }) + '\n');
    assert.equal(await allowed, 'allowed-once');
    // Anything but an explicit allow is a rejection.
    const rejected = approve({ agent: dsh.agent, toolName: 'bash' }, fallthrough);
    socket.write(JSON.stringify({ answer: (await next()).questionId, result: 'maybe' }) + '\n');
    assert.equal(await rejected, 'rejected');
    // An aborted approval is cancelled and withdrawn.
    const controller = new AbortController();
    const cancelled = approve({ agent: dsh.agent, toolName: 'bash', signal: controller.signal }, fallthrough);
    await next();
    controller.abort();
    assert.equal(await cancelled, 'cancelled');
    assert.equal((await next()).event, 'question-cancelled');
  } finally { await stop(server, dsh); }
});


function contextFixture() {
  let notify;
  const events = [
    { seq: 0, type: 'request/context', data: { provider: 'route', model: 'm1', contextWindow: 1000 } },
    { seq: 1, type: 'system/message', data: { content: [{ text: 'private-system-text' }] } },
    { seq: 2, type: 'user/message', data: { source: { kind: 'agent-instructions', changes: [{ action: 'add', path: '/repo/AGENTS.md' }] }, content: [{ text: 'private-instructions' }] } },
    { seq: 3, type: 'assistant/message', data: { usage: { inputTokens: 200 }, content: [{ text: 'private-response' }] } },
  ];
  const values = { contextBreakdown: { systemTokens: 10, toolsTokens: 40, messageTokens: 50 }, contextPressure: { projectedTokens: 210, pressureTokens: 200, contextWindow: 1000 } };
  const measurement = { nodes: [{ seq: 1, heuristicTokens: 10 }, { seq: 2, heuristicTokens: 30 }, { seq: 3, heuristicTokens: 20 }] };
  const dsh = fakeDsh({
    tokenMeter: { measure: () => measurement },
    compaction: { config: { auto: true, thresholdRatio: 0.75, modelPolicies: [{ provider: 'route', model: 'm2', thresholdRatio: 0.6 }] } },
    sessionProjections: {
      onChanged: fn => { notify = fn; },
      snapshot: () => ({ asOfSeq: events.length - 1, values }),
    },
  });
  Object.assign(dsh.agent.session, {
    snapshotEvents: from => events.slice(from), eventAt: seq => events[seq],
    requestHeader: () => ({ config: { model: 'm1' }, tools: [{ name: 'mcp__docs__lookup', description: 'private-schema' }] }),
  });
  return { dsh, events, values, measurement, notify: (...args) => notify(...args) };
}

test('context is opt-in, root-only, coalesced, and exports counts and provenance only', async () => {
  for (const enabled of [false, true]) {
    const { server, port, connection } = await supervisor();
    const { dsh, notify } = contextFixture();
    try {
      apply(dsh.ctx, { port, token: 't', contextInspection: enabled });
      dsh.ready();
      const socket = await connection, next = lines(socket);
      assert.equal((await next()).hello.features.contextInspection, true);
      notify({ id: 'child' }, 'contextPressure');
      notify(dsh.agent.session, 'contextBreakdown');
      notify(dsh.agent.session, 'contextPressure');
      // The command reply fences the queued microtask, so disabled mode must
      // produce no context notification and enabled mode exactly one.
      socket.write(JSON.stringify({ id: 1, method: 'context', params: { sessionId: 'session-1' } }) + '\n');
      if (enabled) {
        const event = await next();
        assert.equal(event.event, 'context');
        assert.equal(event.sessionId, 'session-1');
        assert.equal(event.value.pressure.projectedTokens, 210);
      }
      const reply = await next();
      assert.equal(reply.id, 1);
      if (!enabled) assert.equal(reply.result, null);
      else {
        assert.equal(reply.result.nodes[1].path, '/repo/AGENTS.md');
        assert.equal(reply.result.stale, false);
        assert.equal(reply.result.autoCompactRatio, 0.75);
        assert.equal(JSON.stringify(reply).includes('private-'), false);
      }
    } finally { await stop(server, dsh); }
  }
});

test('context follows native replacement and waits for usage on a changed route', async () => {
  const { createContextReader } = await import('../crates/runtime/src/acp/deepseek-bridge.mjs');
  const { dsh, events, values, measurement } = contextFixture();
  const read = createContextReader(dsh.ctx);
  assert.equal(read(dsh.agent).stale, false);
  events.push({ seq: 4, type: 'request/context', data: { provider: 'route', model: 'm2', contextWindow: 2000 } });
  values.contextPressure.contextWindow = 2000;
  assert.equal(read(dsh.agent).stale, true);
  assert.equal(read(dsh.agent).model, 'm2');
  assert.equal(read(dsh.agent).autoCompactRatio, 0.6);
  events.push({ seq: 5, type: 'assistant/attempt', data: { stream: [{ type: 'usage', usage: { inputTokens: 50 } }] } });
  assert.equal(read(dsh.agent).stale, false);
  // The native meter chooses retained nodes after replacement. The bridge
  // must not resurrect the instruction file from the historical event log.
  measurement.nodes = [{ seq: 1, heuristicTokens: 10 }, { seq: 5, heuristicTokens: 5 }];
  values.contextBreakdown.messageTokens = 5;
  values.contextPressure.projectedTokens = 55;
  const compacted = read(dsh.agent);
  assert.equal(compacted.pressure.projectedTokens, 55);
  assert.equal(compacted.nodes.some(n => n.path), false);
  assert.deepEqual(compacted.nodes.map(n => n.seq), [1, 5]);
});
