// Pockymoe bridge inside the ACP-owned DSH process (inserted with --patch).
// ACP keeps prompts, tools, permissions and cancellation. This reverse channel
// carries startup metadata, an allowlisted subset of DSH's own Remote API,
// session projection views, live assistant text and human questions. It never
// writes stdout and never exports plugin configuration, environment or secrets.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { createServer, request as httpRequest } from 'node:http';
import { pipeline } from 'node:stream';
import { connect } from 'node:net';
import { randomUUID } from 'node:crypto';
import { dirname, join } from 'node:path';

export const name = 'remote-codex-bridge';
export const inject = ['llm', 'appReady'];

const PROTOCOL = 1;
const MAX_LINE = 1 << 20;
const STREAM_FLUSH_MS = 40;
const PROJECTIONS = ['plan', 'goal', 'todos', 'permissions', 'agentPreset'];
const COMMAND = /^\/([a-z][a-z0-9_-]*)(?=$|\s)/;
// The native console proxy answers loopback authorities only; Relay port
// previews already present 127.0.0.1:<port> (DNS-rebound names get 403).
const CONSOLE_HOST = /^(127\.0\.0\.1|localhost|\[::1\]):\d+$/i;
const REMOTES = new Set([
  'commands/list', 'goals/get', 'goals/pause', 'goals/resume', 'goals/clear',
  'permissionPresets/catalog', 'pluginManager/listPlugins', 'pluginManager/listBundles',
  'pluginManager/setPluginEnabled', 'pluginManager/setBundleEnabled', 'llm/listConfigurableProviders',
]);
const MUTATIONS = new Set(['pluginManager/setPluginEnabled', 'pluginManager/setBundleEnabled']);
// Another application bundle in the ACP profile would stop every session from starting.
const APP_BUNDLE = /^@deepseek-ai\/dsh-(base|acp-app|web-app|headless|sdk-app|sdk-minimal)$/;
const BACKUPS_KEPT = 10;
// Profile settings the panel may change, with their value types. Values must
// be JSON scalars: `!!js` strings and `__jsExpr` objects would execute in DSH.
const SETTINGS = {
  permission: { defaultPreset: 'string' },
  'agent-loop': { maxParallelToolCalls: 'number' },
  subagent: { maxDepth: 'number', maxActiveSubagents: 'number' },
  'bash-sandbox': { timeoutMs: 'number', maxOutputBytes: 'number' },
  'llm-deepseek': { reasoningEffort: 'string', maxTokens: 'number' },
  'session-log-deepseek': { enabled: 'boolean' },
};

export function apply(ctx, config) {
  let socket = null;
  let questionId = 0;
  const questions = new Map();
  const streams = new Map();
  const changed = new Map();
  const send = message => {
    if (socket && !socket.destroyed) socket.write(JSON.stringify(message) + '\n');
  };
  const root = agent => ctx.get('agents')?.roots().includes(agent) ?? false;
  // Ownership is direct-only; walk it so background subagents keep a session busy.
  const live = () => {
    const agents = ctx.get('agents');
    return typeof agents?.list === 'function' && typeof agents.isOwnedBy === 'function'
      ? { list: agents.list(), owns: (owner, id) => agents.isOwnedBy(id, owner) }
      : { list: [], owns: () => false };
  };
  const rootOf = agent => {
    const { list, owns } = live();
    for (let current = agent, depth = 0; current && depth < 32; depth += 1) {
      if (root(current)) return current;
      current = list.find(candidate => owns(candidate, current.id));
    }
    return undefined;
  };
  const running = (top, changedAgent, changedStatus) => {
    const { list, owns } = live();
    const family = [top];
    for (let index = 0; index < family.length; index += 1) {
      for (const candidate of list) {
        if (!family.includes(candidate) && owns(family[index], candidate.id)) family.push(candidate);
      }
    }
    return family.some(member => (member === changedAgent ? changedStatus : member.status) === 'running');
  };
  // Run modes: the native Web composition moves model tools into agent presets,
  // and ACP creates agents without one. Bind before the session is published.
  ctx.on('agent/created', async ({ agent }) => {
    const registry = ctx.get('agentPresets');
    const projections = ctx.get('sessionProjections');
    if (!registry || !projections || !root(agent)) return;
    const recorded = projections.snapshot(agent.session, ['agentPreset']).values.agentPreset ?? null;
    if (recorded) return void await registry.recompose(agent.ctx, recorded);
    if (turnStarted(projections, agent)) {
      // A session from before run modes keeps its log; give it the default tools.
      return void await registry.recompose(agent.ctx, registry.defaultId);
    }
    await registry.select(agent, config.preset || registry.defaultId);
  });
  // Billing usage: DSH's ACP reports only context occupancy. Each model
  // call's tokens count toward the root session's turn, subagents included.
  let usageReports = 0;
  ctx.on('session/event', (session, event) => {
    const usage = event?.type === 'assistant/message' ? event.data?.usage : undefined;
    if (!usage) return;
    const owner = live().list.find(agent => agent.session === session);
    const top = owner && rootOf(owner);
    if (!top) return;
    usageReports += 1;
    send({ event: 'usage', sessionId: top.id, reportId: `${session.id}:${event.seq ?? `n${usageReports}`}`, usage });
  });
  let consoleServer = null;
  const busy = new Map();
  const report = (top, changedAgent, changedStatus) => {
    const status = running(top, changedAgent, changedStatus) ? 'running' : 'idle';
    if (busy.get(top.id) === status) return;
    busy.set(top.id, status);
    send({ event: 'status', sessionId: String(top.id), status });
  };

  const flush = attemptId => {
    const stream = streams.get(attemptId);
    if (!stream?.text) return;
    send({ event: 'stream', sessionId: stream.sessionId, attemptId, kind: stream.kind, text: stream.text });
    stream.text = '';
  };
  ctx.on('agent/assistant-stream', ({ agent, frame }) => {
    if (!root(agent)) return;
    const sessionId = String(agent.id);
    if (frame.type === 'start') {
      streams.set(frame.attemptId, { sessionId, kind: null, text: '', timer: null });
      send({ event: 'stream', sessionId, attemptId: frame.attemptId, kind: 'start', step: frame.step });
    } else if (frame.type === 'chunk') {
      const kind = { 'text-delta': 'text', 'reasoning-delta': 'reasoning' }[frame.chunk.type];
      const stream = streams.get(frame.attemptId);
      if (!kind || !stream) return;
      if (stream.kind !== kind) flush(frame.attemptId);
      stream.kind = kind;
      stream.text += frame.chunk.text;
      stream.timer ??= setTimeout(() => { stream.timer = null; flush(frame.attemptId); }, STREAM_FLUSH_MS);
    } else if (frame.type === 'end') {
      const stream = streams.get(frame.attemptId);
      if (stream?.timer) clearTimeout(stream.timer);
      flush(frame.attemptId);
      streams.delete(frame.attemptId);
      const outcome = frame.outcome.kind === 'committed' && frame.outcome.eventType === 'assistant/message'
        ? 'committed' : 'discarded';
      send({ event: 'stream', sessionId, attemptId: frame.attemptId, kind: 'end', outcome });
    }
  });
  ctx.on('agent/status', ({ agent, status }) => {
    const top = rootOf(agent);
    if (top) report(top, agent, status);
  });
  ctx.on('agent/disposed', ({ agent }) => {
    for (const top of ctx.get('agents')?.roots() ?? []) if (top !== agent) report(top, agent, 'idle');
  });
  ctx.inject(['sessionProjections'], projectionCtx => {
    projectionCtx.sessionProjections.onChanged((session, key) => {
      if (!PROJECTIONS.includes(key)) return;
      // Coalesce one committed event's units; send validated client views.
      if (!changed.has(session)) queueMicrotask(() => {
        const keys = [...(changed.get(session) ?? [])];
        changed.delete(session);
        const agent = ctx.get('agents')?.roots().find(candidate => candidate.session === session);
        if (!agent) return;
        const { values } = projectionCtx.sessionProjections.snapshot(session, keys);
        for (const [key, value] of Object.entries(values)) {
          send({ event: 'projection', sessionId: String(agent.id), key, value: value ?? null });
        }
      });
      changed.set(session, (changed.get(session) ?? new Set()).add(key));
    });
  });
  // Questions and tool approvals for a session or its subagents go to the
  // Pockymoe turn. DSH's Web host would otherwise hold them for a Web
  // client that is not there, and the session would wait forever.
  const ask = (signal, message) => new Promise((resolve, reject) => {
    const id = ++questionId;
    const abort = () => {
      questions.delete(id);
      send({ event: 'question-cancelled', questionId: id });
      reject(signal.reason ?? new Error('question aborted'));
    };
    if (signal?.aborted) return abort();
    signal?.addEventListener('abort', abort, { once: true });
    questions.set(id, {
      resolve: value => { signal?.removeEventListener('abort', abort); resolve(value); },
      reject: error => { signal?.removeEventListener('abort', abort); reject(error); },
    });
    send({ ...message, questionId: id });
  });
  const answerable = agent => socket && !socket.destroyed && agent ? rootOf(agent) : undefined;
  // Without an answerer, DSH rejects plan reviews and questions as NO_PROVIDER.
  ctx.on('user-questions/request', (request, next) => {
    const top = answerable(request.agent);
    if (!top) return next();
    return ask(request.signal, { event: 'question', sessionId: String(top.id), questions: request.questions });
  });
  ctx.on('approval/request', (request, next) => {
    const top = answerable(request.agent);
    if (!top) return next();
    return ask(request.signal, {
      event: 'approval',
      sessionId: String(top.id),
      request: {
        toolName: request.toolName,
        callId: request.callId ?? null,
        reason: request.displayReason?.en ?? request.reason ?? null,
        subagent: request.agent !== top,
      },
    }).then(outcome => (outcome === 'allowed-once' ? 'allowed-once' : 'rejected'), () => 'cancelled');
  });

  const dispose = ctx.appReady.onReady(() => {
    void snapshot(ctx).then(data => {
      socket = connect({ host: '127.0.0.1', port: config.port });
      // Decode across reads: a split UTF-8 character must not become U+FFFD.
      socket.setEncoding('utf8');
      socket.on('error', () => {});
      socket.on('close', () => {
        for (const question of questions.values()) question.reject(new Error('Pockymoe disconnected'));
        questions.clear();
      });
      socket.on('connect', () => send({ token: config.token, hello: data }));
      let buffer = '';
      socket.on('data', chunk => {
        buffer += chunk;
        if (buffer.length > MAX_LINE && !buffer.includes('\n')) return socket.destroy();
        let index;
        while ((index = buffer.indexOf('\n')) >= 0) {
          const line = buffer.slice(0, index);
          buffer = buffer.slice(index + 1);
          let message;
          try { message = JSON.parse(line); } catch { continue; }
          receive(message);
        }
      });
    }, error => {
      // Connect only to report the failure; the supervisor rejects the session.
      socket = connect({ host: '127.0.0.1', port: config.port });
      socket.on('error', () => {});
      socket.on('connect', () => socket.end(JSON.stringify({ token: config.token, error: String(error?.message ?? error) }) + '\n'));
    });
  });
  ctx.on('dispose', () => {
    dispose();
    socket?.destroy();
    void consoleServer?.then(server => server.shutdown(), () => {});
  });

  function receive(message) {
    if (message.answer !== undefined) {
      const question = questions.get(message.answer);
      questions.delete(message.answer);
      if (!question) return;
      if (message.error) question.reject(new Error(String(message.error)));
      else question.resolve(message.result);
      return;
    }
    if (message.id === undefined) return;
    void call(message.method, message.params ?? {}).then(
      result => send({ id: message.id, result: result ?? null }),
      error => send({ id: message.id, error: { message: String(error?.message ?? error), code: error?.code ?? null } }),
    );
  }

  async function call(method, params) {
    switch (method) {
      case 'snapshot': return snapshot(ctx);
      case 'session': return session(params.sessionId);
      case 'invoke': {
        const endpoint = `${params.namespace}/${params.method}`;
        if (!REMOTES.has(endpoint)) throw new Error(`Remote ${endpoint} is not available through Pockymoe`);
        if (endpoint === 'pluginManager/setBundleEnabled' && APP_BUNDLE.test(String(params.args?.name))) {
          throw new Error(`${params.args.name} is an application bundle; the ACP profile cannot switch it`);
        }
        if (MUTATIONS.has(endpoint)) backupProfile(ctx);
        return remote(ctx, params.namespace, params.method, params.args ?? {});
      }
      case 'command': {
        // Any command DSH registered for this session, including plugin commands.
        const line = String(params.line ?? '').trim();
        const name = COMMAND.exec(line)?.[1];
        const registered = await remote(ctx, 'commands', 'list', { agentId: params.sessionId });
        if (!name || !registered.some(command => command.name === name)) {
          throw new Error(`DSH has no command ${line.split(/\s/)[0] || line}`);
        }
        return remote(ctx, 'commands', 'execute', { agentId: params.sessionId, line, submittedAttachments: [] });
      }
      case 'selectPreset': {
        const registry = ctx.get('agentPresets');
        if (!registry) throw new Error('This DSH composition has no run modes');
        return { agentPreset: await registry.select(liveRoot(params.sessionId), String(params.preset)) };
      }
      case 'console': return openConsole();
      case 'rename': {
        // An explicit title pins it: DSH stops generating one from the prompt.
        const titles = ctx.get('sessionTitle');
        if (typeof titles?.rename !== 'function') throw new Error('This DSH composition has no session titles');
        const { session } = liveRoot(params.sessionId);
        const title = String(params.title ?? '').trim();
        if (titles.get?.(session)?.title === title) return { title };
        return { title: titles.rename(session, title).title };
      }
      case 'settings': return settings(ctx);
      case 'updateSetting': return updateSetting(ctx, params);
      case 'fork': return fork(ctx, params);
      default: throw new Error(`Unknown bridge method ${method}`);
    }
  }

  function liveRoot(sessionId) {
    const agent = ctx.get('agents')?.get(sessionId);
    if (!agent || !root(agent)) throw new Error(`DSH session ${sessionId} is not live`);
    return agent;
  }

  async function session(sessionId) {
    const agent = liveRoot(sessionId);
    const projector = ctx.get('sessionProjections');
    const projections = projector?.snapshot(agent.session, PROJECTIONS).values ?? {};
    // Unknown (null) rather than empty when DSH cannot list them right now.
    const commands = await remote(ctx, 'commands', 'list', { agentId: sessionId }).catch(() => null);
    return {
      projections,
      running: running(agent),
      presetLocked: projector ? turnStarted(projector, agent) : true,
      commands: commands?.map(({ name, description, input }) => ({
        name, description: description ?? '', hint: input?.hint ?? null,
      })),
    };
  }

  // The same-process native Web UI, reached through a loopback proxy so Relay
  // preview hosts pass DSH's Host/Origin fence; DSH's own token still applies.
  async function openConsole() {
    const web = ctx.get('webServer');
    const connection = ctx.get('connection');
    if (!web?.port || typeof connection?.authenticatedUrl !== 'function') {
      throw new Error('This DSH composition has no native console');
    }
    // One proxy per process, even when two panels ask at once.
    consoleServer ??= startConsoleProxy(web.port).catch(error => { consoleServer = null; throw error; });
    const port = (await consoleServer).address().port;
    const url = new URL(connection.authenticatedUrl(`http://127.0.0.1:${port}/`));
    return { port, path: `${url.pathname}${url.search}` };
  }
}

function turnStarted(projections, agent) {
  // Without the turn-boundary view a session cannot be shown to be unstarted.
  if (typeof projections.stateOf !== 'function') return true;
  const boundary = projections.stateOf(agent.session, 'turnBoundary');
  return boundary !== undefined && (boundary.openTurnStartSeq !== null || boundary.lastTurn > 0);
}

function loginRedirect(req, reply) {
  return req.method === 'GET' && /^\/\?/.test(req.url ?? '') && reply.statusCode === 303
    && reply.headers.location === './' && Boolean(reply.headers['set-cookie']);
}

function startConsoleProxy(targetPort) {
  const authority = `127.0.0.1:${targetPort}`;
  const forward = headers => {
    if (!CONSOLE_HOST.test(String(headers.host ?? ''))) return null;
    const next = { ...headers, host: authority };
    // DSH requires Origin == Host. Rewrite same-origin requests only; a page
    // from another origin keeps its Origin and is refused by DSH.
    if (headers.origin) {
      try {
        if (new URL(headers.origin).host === headers.host) next.origin = `http://${authority}`;
      } catch { /* a malformed Origin stays as sent and fails DSH's check */ }
    }
    return next;
  };
  const sockets = new Set();
  const track = socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); };
  const server = createServer((req, res) => {
    const headers = forward(req.headers);
    if (!headers) return res.writeHead(403).end();
    const upstream = httpRequest(
      { host: '127.0.0.1', port: targetPort, method: req.method, path: req.url, headers },
      reply => {
        if (loginRedirect(req, reply)) {
          // DSH's token login sets a SameSite=Strict cookie and 303s to './'.
          // When the console was opened from another site (a Relay app domain,
          // another host name), the browser drops that cookie on the redirect.
          // Continue from the page itself instead, which is same-site.
          reply.resume();
          const { location: _, 'content-length': __, ...kept } = reply.headers;
          res.writeHead(200, { ...kept, 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store', 'referrer-policy': 'no-referrer' });
          return res.end('<!doctype html><meta http-equiv="refresh" content="0;url=./"><title>DeepSeek Harness</title><a href="./">DeepSeek Harness</a>');
        }
        res.writeHead(reply.statusCode ?? 502, reply.headers);
        // A failed or abandoned body ends both sides (e.g. EventSource streams).
        pipeline(reply, res, () => {});
      },
    );
    upstream.on('error', () => { if (!res.headersSent) res.writeHead(502).end(); else res.destroy(); });
    res.on('close', () => upstream.destroy());
    pipeline(req, upstream, () => {});
  });
  server.on('upgrade', (req, client, head) => {
    const headers = forward(req.headers);
    if (!headers) return client.destroy();
    track(client);
    const upstream = connect({ host: '127.0.0.1', port: targetPort }, () => {
      const lines = Object.entries(headers).flatMap(([key, value]) =>
        (Array.isArray(value) ? value : [value]).map(item => `${key}: ${item}`));
      upstream.write(`${req.method} ${req.url} HTTP/${req.httpVersion}\r\n${lines.join('\r\n')}\r\n\r\n`);
      if (head?.length) upstream.write(head);
      upstream.pipe(client);
      client.pipe(upstream);
    });
    track(upstream);
    upstream.on('error', () => client.destroy());
    upstream.on('close', () => client.destroy());
    client.on('error', () => upstream.destroy());
    client.on('close', () => upstream.destroy());
  });
  server.shutdown = () => {
    server.close();
    server.closeAllConnections();
    for (const socket of sockets) socket.destroy();
  };
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve(server));
  });
}

// Fork the way DSH's session controller does (an exact event prefix seeds a
// new root session), but without its parent link: DSH's ACP resumes only
// sessions without one. The new session is released at once so the fork's own
// process can open it. A rollback drops that many of the latest user turns:
// Pockymoe's turns are DSH turns carrying a user message, while goal
// continuations, compaction and restart context add no Pockymoe turn.
async function fork(ctx, { sessionId, rollbackCount = 0 }) {
  const query = ctx.get('sessionQuery');
  const agents = ctx.get('agents');
  if (!query || typeof agents?.create !== 'function') throw new Error('This DSH composition cannot fork sessions');
  const observed = await query.observeSession(sessionId);
  try {
    const events = observed.events;
    const boundary = forkBoundary(events, Number(rollbackCount) || 0, sessionId);
    const seed = await forkSeed(events, boundary);
    const preset = observed.projections?.values?.agentPreset ?? undefined;
    const selection = ctx.get('agentDefaultModel')?.currentSelection?.();
    const child = randomUUID();
    const handle = await agents.create({
      sessionId: child,
      seed,
      inheritedEventCount: boundary + 1,
      meta: {
        ...(observed.header?.cwd === undefined ? {} : { cwd: observed.header.cwd }),
        isSeeded: true,
        ...(preset === undefined ? {} : { agentPreset: preset }),
      },
      ...(selection?.provider && selection?.model
        ? { agentOptions: { provider: selection.provider, model: selection.model } } : {}),
    });
    await handle.dispose();
    return { sessionId: child };
  } finally {
    observed[Symbol.dispose]?.();
  }
}

/** The inclusive event seq a fork keeps: the end of the chosen user turn and
    what closes it, up to the next prompt (as DSH's latest-turn fork keeps). */
function forkBoundary(events, rollback, sessionId) {
  const turns = [];
  let open = null;
  for (const event of events) {
    if (event.type === 'turn/start') open = { user: false };
    else if (open && event.type === 'user/message' && event.surfaceOp === 'append'
      && event.data?.source?.kind === 'user') open.user = true;
    else if (open && event.type === 'turn/end') {
      if (open.user) turns.push(event.seq);
      open = null;
    }
  }
  const end = turns[turns.length - 1 - rollback];
  if (end === undefined) throw new Error(`DSH session ${sessionId} has no turn ${turns.length - rollback} to fork from`);
  let boundary = end;
  for (const next of events.slice(events.findIndex(event => event.seq === end) + 1)) {
    if (next.type === 'turn/start' || (next.type === 'user/message' && next.surfaceOp === 'append')
      || next.type === 'agent/inbox/spliced') break;
    boundary = next.seq;
  }
  return boundary;
}

/** DSH's own seed builder when the installed release exports it; a cut after
    a closed turn needs no synthetic closers, so the fallback only marks it. */
async function forkSeed(events, boundary) {
  try {
    const { buildForkSeed } = await import('@deepseek-ai/dsh-session/fork');
    return buildForkSeed(events, boundary);
  } catch {
    return [...events.slice(0, boundary + 1), {
      type: 'session/end-seed', seq: boundary + 1, time: events[boundary].time, data: { inherited: true },
    }];
  }
}

async function remote(ctx, namespace, method, args = {}) {
  const gateway = ctx.get('typertGateway');
  if (!gateway) throw new Error('DSH Remote gateway is unavailable');
  return gateway.invoke({ namespace, method, args });
}

async function snapshot(ctx) {
  const optional = promise => promise.catch(() => null);
  const registry = ctx.get('agentPresets');
  const [models, presets, plugins, bundles, providers, runModes] = await Promise.all([
    discoverModels(ctx),
    optional(remote(ctx, 'permissionPresets', 'catalog')),
    optional(remote(ctx, 'pluginManager', 'listPlugins')),
    optional(remote(ctx, 'pluginManager', 'listBundles')),
    optional(remote(ctx, 'llm', 'listConfigurableProviders')),
    registry ? optional(registry.list()) : null,
  ]);
  return {
    protocol: PROTOCOL,
    version: dshVersion(),
    profile: ctx.get('profileContext')?.name ?? 'acp',
    models,
    permissionPresets: presets?.options?.map(option => option.value) ?? [],
    defaultPermissionPreset: presets?.defaultPreset ?? null,
    plugins: plugins ? plugins.filter(plugin => plugin.moduleName !== 'cordis:include').map(plugin => ({
      id: plugin.entryId, name: plugin.meta?.title ?? plugin.moduleName, module: plugin.moduleName,
      description: plugin.meta?.description ?? '', enabled: plugin.enabled,
      active: plugin.fiberPhase === 'active', readOnly: plugin.readOnlyReason ?? null,
    })) : loaderPlugins(ctx),
    bundles: (bundles ?? []).map(bundle => ({
      name: bundle.name, version: bundle.version ?? null, description: bundle.description ?? '',
      enabled: bundle.enabled, removable: Boolean(bundle.removable),
      readOnly: bundle.readOnlyReason ?? (APP_BUNDLE.test(bundle.name) ? 'application-bundle' : null),
    })),
    providers: (providers ?? []).map(provider => ({
      id: provider.provider, name: provider.displayName, declared: provider.declared !== false,
    })),
    runModes: (runModes ?? []).map(mode => ({
      id: mode.id, name: mode.name ?? null, description: mode.description ?? null,
      isDefault: mode.id === registry.defaultId,
      // PTC's run_code executes TypeScript in this Node; distro builds can lack it.
      broken: mode.broken
        ?? (mode.id === 'ptc' && !process.features?.typescript ? 'needs Node.js with TypeScript support' : null),
    })),
    features: {
      stream: true,
      questions: true,
      projections: Boolean(ctx.get('sessionProjections')),
      commands: Boolean(ctx.get('commands') && ctx.get('typertGateway')),
      goals: Boolean(ctx.get('goals')),
      permissions: Boolean(presets),
      pluginManager: Boolean(plugins),
      settings: Boolean(ctx.get('settings')),
      runModes: Boolean(registry),
      console: Boolean(ctx.get('webServer') && ctx.get('connection')),
      fork: typeof ctx.get('agents')?.create === 'function' && Boolean(ctx.get('sessionQuery')),
    },
  };
}

async function discoverModels(ctx) {
  const models = [];
  for (const provider of ctx.llm.listProviders()) {
    for (const model of await ctx.llm.listModels(provider.id)) {
      const info = await ctx.llm.resolveModelInfo(provider.id, model.id);
      const reasoning = info?.reasoning;
      const efforts = (reasoning?.efforts ?? []).map(effort => ({
        reasoningEffort: String(effort.id), description: effort.description ?? effort.name ?? String(effort.id),
      }));
      if (reasoning && reasoning.defaultEffort === undefined) {
        efforts.unshift({ reasoningEffort: '', description: 'Provider default' });
      }
      const value = JSON.stringify([provider.id, model.id]);
      models.push({ id: value, model: value, displayName: `${provider.name} · ${model.name}`,
        description: model.description ?? '', isDefault: false, hidden: false,
        supportedReasoningEfforts: efforts,
        defaultReasoningEffort: reasoning ? String(reasoning.defaultEffort ?? '') : null,
        selectionKind: 'model', acpAgent: null });
    }
  }
  return models;
}

function loaderPlugins(ctx) {
  // Pre-plugin-manager DSH: an explicit allowlist of loader entry fields.
  return [...(ctx.loader?.entries() ?? [])].filter(entry => !entry.options.group).map(entry => ({
    id: entry.id, name: entry.options.name, module: entry.options.name, description: '',
    enabled: !entry.disabled, active: !entry.disabled, readOnly: 'unmanaged',
  }));
}

function settings(ctx) {
  const views = ctx.get('settings')?.describe({ redactSecrets: true }) ?? [];
  return views.filter(view => SETTINGS[view.ns]).map(view => ({
    ns: view.ns,
    revision: view.revision,
    fields: Object.entries(SETTINGS[view.ns]).map(([key, type]) => ({
      key,
      type,
      value: scalar(view.value?.[key]),
      overridden: Boolean(view.user && Object.hasOwn(view.user, key)),
    })),
  }));
}

async function updateSetting(ctx, { ns, key, value, revision }) {
  const type = Object.hasOwn(SETTINGS, ns) && Object.hasOwn(SETTINGS[ns], key) ? SETTINGS[ns][key] : null;
  if (!type) throw new Error(`Setting ${ns}.${key} is not editable through Pockymoe`);
  if (value !== null && typeof value !== type) throw new Error(`Setting ${ns}.${key} expects a ${type}`);
  if (type === 'number' && value !== null && !Number.isFinite(value)) throw new Error(`Setting ${ns}.${key} expects a finite number`);
  if (typeof value === 'string' && value.trimStart().startsWith('!!')) throw new Error('YAML tags are not accepted');
  // DSH skips its conflict check only for an absent revision; always send one.
  if (!Number.isSafeInteger(revision)) throw new Error('A settings revision is required');
  const forms = ctx.get('settings');
  if (!forms) throw new Error('DSH settings are unavailable in this profile');
  backupProfile(ctx);
  const op = value === null ? { op: 'unset', path: [key] } : { op: 'set', path: [key], value };
  await forms.mutate(ns, [op], revision);
  return settings(ctx).find(view => view.ns === ns) ?? null;
}

// A profile edit that breaks startup must stay recoverable outside DSH.
function backupProfile(ctx) {
  const profile = ctx.get('profileContext');
  if (!profile?.dir) throw new Error('DSH profile location is unknown; refusing to change it');
  const root = join(profile.dir, '.remote-codex', 'backups');
  const target = join(root, new Date().toISOString().replace(/[:.]/g, '-'));
  mkdirSync(target, { recursive: true, mode: 0o700 });
  for (const file of [profile.patchPath, profile.installAnchor]) {
    if (file && existsSync(file)) copyFileSync(file, join(target, file.split(/[\\/]/).pop()));
  }
  for (const stale of readdirSync(root).sort().slice(0, -BACKUPS_KEPT)) {
    rmSync(join(root, stale), { recursive: true, force: true });
  }
}

function scalar(value) {
  return value === null || ['string', 'number', 'boolean'].includes(typeof value) ? value : null;
}

function dshVersion() {
  try {
    let directory = dirname(realpathSync(process.argv[1]));
    for (let depth = 0; depth < 4; depth += 1, directory = dirname(directory)) {
      try {
        const manifest = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
        if (manifest.name === '@deepseek-ai/dsh') return manifest.version;
      } catch { /* no manifest at this level; keep walking toward the package root */ }
    }
  } catch { /* argv[1] is not a resolvable launcher path */ }
  return null;
}
