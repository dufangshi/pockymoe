import type {
  AutomationDto,
  NativeSubagentDto,
  RelayAccessGrantDto,
  RelayDeviceDto,
  RelaySessionShareDto,
  ShellSessionDto,
  ThreadDto,
  ThreadHistoryItemDto,
  ThreadTurnDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import { TOUR_DEVICE_ID } from './tourDevice';
import {
  DEMO_WORKSPACE,
  sampleAutomations,
  sampleFiles,
  sampleSubagents,
  sampleThreads,
  sampleTurns,
  sampleWorkspaces,
  SAMPLE_ROOT,
  SAMPLE_USER,
  say,
  scriptedReply,
} from './sample';
import { TourShell } from './shell';

/**
 * The tour's sample Supervisor, in this browser tab: the API and socket a real
 * device offers, over sample data kept in memory. Nothing leaves the page.
 */

interface State {
  workspaces: WorkspaceDto[];
  files: Record<string, Record<string, string>>;
  threads: ThreadDto[];
  turns: Record<string, ThreadTurnDto[]>;
  automations: Record<string, AutomationDto[]>;
  subagents: Record<string, NativeSubagentDto[]>;
  shells: Record<string, ShellSessionDto[]>;
  shares: RelaySessionShareDto[];
  grants: RelayAccessGrantDto[];
  saves: Record<string, unknown>;
}

let state: State | null = null;

function fresh(): State {
  return {
    workspaces: sampleWorkspaces(),
    files: sampleFiles(),
    threads: sampleThreads(),
    turns: sampleTurns(),
    automations: sampleAutomations(),
    subagents: sampleSubagents(),
    shells: {},
    shares: [],
    grants: [],
    saves: {},
  };
}

function data() {
  state ??= fresh();
  return state;
}

/** Starts the sample over: the tour's own edits are gone. */
export function resetTourSandbox() {
  for (const timer of running.values()) timer.cancel();
  running.clear();
  state = null;
}

const now = () => new Date().toISOString();
let counter = 0;
const uid = () => {
  counter += 1;
  const tail = `${Date.now().toString(16)}${counter.toString(16)}`.padStart(12, '0').slice(-12);
  return `d0c5a11e-7041-4000-8000-${tail}`;
};

function json(body: unknown, status = 200) {
  return new Response(status === 204 ? null : JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
}

function fail(status: number, message: string, code = status === 404 ? 'not_found' : 'bad_request') {
  return json({ code, message }, status);
}

const unsupported = () => fail(501, say('教程的样例设备不支持这个操作；在你自己的设备上可以正常使用。', 'The tour\'s sample device does not do this; your own devices do.'), 'service_unavailable');

function hash(text: string) {
  let value = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) value = Math.imul(value ^ text.charCodeAt(index), 0x01000193);
  return `sha256:tour${(value >>> 0).toString(16).padStart(8, '0')}`;
}

function language(path: string) {
  const extension = path.split('.').pop() ?? '';
  return ({ ts: 'typescript', tsx: 'typescript', js: 'javascript', json: 'json', md: 'markdown' } as Record<string, string>)[extension] ?? 'plaintext';
}

function workspace(id: string) {
  return data().workspaces.find((item) => item.id === id) ?? null;
}

function thread(id: string) {
  return data().threads.find((item) => item.id === id) ?? null;
}

function body<T>(init: RequestInit): T {
  try {
    return (typeof init.body === 'string' ? JSON.parse(init.body) : {}) as T;
  } catch {
    return {} as T;
  }
}

// ---- Files -----------------------------------------------------------------

function tree(workspaceId: string, base: string) {
  const files = data().files[workspaceId] ?? {};
  const prefix = base ? `${base}/` : '';
  const children = new Map<string, { kind: 'file' | 'directory'; size: number }>();
  for (const [path, content] of Object.entries(files)) {
    if (!path.startsWith(prefix)) continue;
    const [name, ...rest] = path.slice(prefix.length).split('/');
    if (!name) continue;
    if (rest.length) children.set(name, { kind: 'directory', size: 0 });
    else if (!children.has(name)) children.set(name, { kind: 'file', size: new Blob([content]).size });
  }
  const at = now();
  const nodes = [...children.entries()]
    .sort(([a, left], [b, right]) => (left.kind === right.kind ? a.localeCompare(b) : left.kind === 'directory' ? -1 : 1))
    .map(([name, node]) => ({
      name, path: `${prefix}${name}`, kind: node.kind, readonly: false, hasChildren: node.kind === 'directory', childrenLoaded: false,
      mode: node.kind === 'directory' ? 'rwxr-xr-x' : 'rw-r--r--', createdAt: at, modifiedAt: at, accessedAt: at,
      ...(node.kind === 'file' ? { size: node.size } : {}),
    }));
  const label = workspace(workspaceId)?.label ?? '';
  return { name: base ? base.split('/').pop() : label, path: base, kind: 'directory', hasChildren: nodes.length > 0, childrenLoaded: true, children: nodes };
}

function snapshot(workspaceId: string, path: string) {
  const content = data().files[workspaceId]?.[path];
  if (content === undefined) return null;
  return {
    path, name: path.split('/').pop() ?? path, language: language(path), workspaceRevision: hash(Object.values(data().files[workspaceId] ?? {}).join('\0')),
    fileIdentity: hash(`${workspaceId}:${path}`), contentHash: hash(content), content, size: new Blob([content]).size,
    encoding: 'utf-8', bom: false, eol: 'lf', readOnlyReason: null, truncated: false,
  };
}

function files(workspaceId: string, rest: string[], url: URL, init: RequestInit, method: string) {
  const store = data().files[workspaceId];
  if (!store) return fail(404, 'workspace not found');
  const path = (url.searchParams.get('path') ?? '').replace(/^\/+/, '');
  const [action, extra] = rest;
  if (action === 'tree') return json(tree(workspaceId, path));
  if (action === 'preview') {
    const content = store[path];
    if (content === undefined) return fail(404, 'file not found');
    return json({ path, name: path.split('/').pop(), content, language: language(path), size: content.length, nextOffset: content.length, truncated: false });
  }
  if (action === 'document') return snapshot(workspaceId, path) ? json(snapshot(workspaceId, path)) : fail(404, 'file not found');
  if (action === 'raw' || action === 'download') {
    const content = store[path];
    if (content === undefined) return fail(404, 'file not found');
    return new Response(content, { headers: { 'content-type': 'text/plain; charset=utf-8', 'content-disposition': `attachment; filename="${encodeURIComponent(path.split('/').pop() ?? 'file')}"` } });
  }
  if (action === 'save' && method === 'POST') {
    const input = body<{ path: string; expectedHash: string; content: string; draftRevision: number; operationId: string }>(init);
    const current = snapshot(workspaceId, input.path);
    let receipt: Record<string, unknown>;
    if (!current) receipt = { status: 'failedBeforeWrite', operationId: input.operationId, draftRevision: input.draftRevision, path: input.path, code: 'not_found', message: 'file not found' };
    else if (current.contentHash !== input.expectedHash) receipt = { status: 'conflict', operationId: input.operationId, draftRevision: input.draftRevision, path: input.path, snapshot: current };
    else {
      store[input.path] = input.content;
      const saved = snapshot(workspaceId, input.path)!;
      receipt = { status: 'saved', operationId: input.operationId, draftRevision: input.draftRevision, path: input.path, contentHash: saved.contentHash, fileIdentity: saved.fileIdentity, size: saved.size, workspaceRevision: saved.workspaceRevision, encoding: 'utf-8', bom: false, eol: 'lf', snapshot: saved };
    }
    data().saves[input.operationId] = receipt;
    return json(receipt);
  }
  if (action === 'operations' && extra) return data().saves[extra] ? json(data().saves[extra]) : fail(404, 'operation not found');
  if (!action && method === 'POST') {
    const target = body<{ path: string }>(init).path.replace(/^\/+/, '');
    if (!target || store[target] !== undefined) return fail(409, say('文件已存在', 'That file already exists'), 'conflict');
    store[target] = '';
    return json({ path: target });
  }
  if (!action && method === 'DELETE') {
    for (const key of Object.keys(store)) if (key === path || key.startsWith(`${path}/`)) delete store[key];
    return json({ path });
  }
  if (action === 'move' && method === 'PATCH') {
    const { fromPath, toPath } = body<{ fromPath: string; toPath: string }>(init);
    for (const key of Object.keys(store)) {
      if (key !== fromPath && !key.startsWith(`${fromPath}/`)) continue;
      store[`${toPath}${key.slice(fromPath.length)}`] = store[key]!;
      delete store[key];
    }
    return json({ path: toPath });
  }
  return unsupported();
}

// ---- Threads ---------------------------------------------------------------

function listThreads(includeAgents: boolean, workspaceId?: string) {
  return data().threads.filter((item) => (includeAgents || !item.parentThreadId) && (!workspaceId || item.workspaceId === workspaceId));
}

function detail(id: string, url: URL) {
  const current = thread(id);
  if (!current) return fail(404, 'thread not found');
  const turns = data().turns[id] ?? [];
  const limit = Number(url.searchParams.get('limit') ?? 3) || 3;
  const before = url.searchParams.get('beforeTurnId');
  const end = before ? Math.max(0, turns.findIndex((turn) => turn.id === before)) : turns.length;
  return json({
    thread: current, workspace: workspace(current.workspaceId), workspacePathStatus: 'present',
    turns: turns.slice(Math.max(0, end - limit), end), totalTurnCount: turns.length,
    pendingRequests: [], pendingSteers: [], activeSubagents: [], activityNotes: [],
  });
}

function createThread(input: { workspaceId: string; title?: string; provider?: string; model?: string; agentId?: string; reasoningEffort?: string | null }) {
  if (!workspace(input.workspaceId)) return null;
  const at = now();
  const created = {
    id: uid(), workspaceId: input.workspaceId, title: input.title?.trim() || say('新对话', 'New conversation'), provider: input.provider ?? 'codex',
    providerSessionId: null, source: 'supervisor', model: input.model ?? 'gpt-5.4', reasoningEffort: input.reasoningEffort ?? 'medium',
    collaborationMode: 'default', approvalMode: 'yolo', status: 'idle', summaryText: null, lastError: null, activeTurnId: null,
    isLoaded: true, isPinned: false, createdAt: at, updatedAt: at, lastTurnStartedAt: null, lastTurnCompletedAt: null, lineageDepth: 0,
    ...(input.agentId ? { agentId: input.agentId } : {}),
  } as unknown as ThreadDto;
  data().threads.unshift(created);
  data().turns[created.id] = [];
  return created;
}

function update(id: string, patch: Partial<ThreadDto>, reason?: string) {
  const current = thread(id);
  if (!current) return null;
  Object.assign(current, patch, { updatedAt: now() });
  broadcast({ type: 'thread.updated', threadId: id, timestamp: now(), payload: { ...patch, ...(reason ? { reason } : {}) } });
  return current;
}

const running = new Map<string, { cancel: () => void }>();

/** A sample turn: progress, one command and a streamed answer, about ten seconds long so Stop can be tried. */
function startTurn(id: string, prompt: string) {
  const current = thread(id)!;
  const turnId = uid();
  const turn: ThreadTurnDto = {
    id: turnId, status: 'inProgress', startedAt: now(), completedAt: null, error: null, model: current.model,
    items: [{ id: `${turnId}:user`, kind: 'userMessage', origin: 'user', text: prompt, createdAt: now() }],
  };
  (data().turns[id] ??= []).push(turn);
  const reply = scriptedReply(prompt);
  const timers: number[] = [];
  const emit = (type: string, payload: unknown) => broadcast({ type, threadId: id, timestamp: now(), payload });
  const at = (delay: number, run: () => void) => timers.push(window.setTimeout(run, delay));
  const add = (item: ThreadHistoryItemDto, started = false) => {
    const index = turn.items.findIndex((entry) => entry.id === item.id);
    if (index >= 0) turn.items[index] = item;
    else turn.items.push(item);
    emit(started ? 'thread.item.started' : 'thread.item.completed', { turnId, item });
  };
  const finish = (status: 'completed' | 'interrupted') => {
    for (const timer of timers) window.clearTimeout(timer);
    running.delete(id);
    turn.status = status;
    turn.completedAt = now();
    emit('thread.turn.completed', { turnId, status, error: null });
    update(id, { status: 'idle', activeTurnId: null, lastTurnCompletedAt: turn.completedAt } as Partial<ThreadDto>);
  };
  running.set(id, { cancel: () => finish('interrupted') });
  update(id, { status: 'running', activeTurnId: turnId, lastTurnStartedAt: turn.startedAt, title: current.title } as Partial<ThreadDto>);
  emit('thread.turn.started', { turnId });
  at(1200, () => add({ id: `${turnId}:progress`, kind: 'agentMessage', responsePhase: 'commentary', status: 'completed', text: reply.commentary, createdAt: now() }));
  at(2400, () => add({ id: `${turnId}:command`, kind: 'commandExecution', status: 'inProgress', text: reply.command, createdAt: now() }, true));
  at(6500, () => add({
    id: `${turnId}:command`, kind: 'commandExecution', status: 'completed', text: reply.command, createdAt: now(),
    detailText: `Tool: exec_command\n\nKind: execute\n\nStatus: completed\n\nInput:\n${JSON.stringify({ cmd: reply.command })}\n\nOutput:\n${JSON.stringify(reply.commandOutput)}`,
  }));
  const chunks = reply.answer.match(/[\s\S]{1,6}/g) ?? [];
  chunks.forEach((delta, sequence) => at(7000 + sequence * 160, () => emit('thread.output.delta', { turnId, itemId: `${turnId}:answer`, sequence, delta, createdAt: now() })));
  at(7200 + chunks.length * 160, () => {
    add({ id: `${turnId}:answer`, kind: 'agentMessage', responsePhase: 'final', status: 'completed', text: reply.answer, createdAt: now() });
    finish('completed');
  });
  return current;
}

function threadRoutes(id: string, rest: string[], url: URL, init: RequestInit, method: string): Response {
  const current = thread(id);
  if (!current) return fail(404, 'thread not found');
  const [action, second, third] = rest;
  if (!action) {
    if (method === 'GET') return detail(id, url);
    if (method === 'PATCH') return json(update(id, { title: body<{ title: string }>(init).title } as Partial<ThreadDto>, 'renamed'));
    if (method === 'DELETE') {
      data().threads = data().threads.filter((item) => item.id !== id);
      return json({ id });
    }
  }
  switch (action) {
    case 'group':
      return json(data().threads.filter((item) => item.id === (current.rootThreadId ?? id) || item.rootThreadId === (current.rootThreadId ?? id)));
    case 'turns': {
      const turn = (data().turns[id] ?? []).find((item) => item.id === second);
      return turn ? json(turn) : fail(404, 'turn not found');
    }
    case 'items': {
      const item = (data().turns[id] ?? []).flatMap((turn) => turn.items).find((entry) => entry.id === decodeURIComponent(second ?? ''));
      return item ? json({ id: item.id, kind: item.kind, title: item.text, text: item.detailText ?? item.text }) : fail(404, 'item not found');
    }
    case 'models':
      return json(models());
    case 'capabilities':
      return json({ agentId: current.provider, provider: current.provider, availability: 'ready', negotiated: null, effectiveCapabilities: capabilities(), toolboxItems: [] });
    case 'plugins':
      return json(plugins());
    case 'agents':
      return json([]);
    case 'prompt': {
      if (method !== 'POST') break;
      if (running.has(id)) return fail(409, say('样例 Agent 还在回复，请等它说完或先停止。', 'The sample agent is still replying; wait or stop it first.'), 'conflict');
      const prompt = typeof init.body === 'string' ? body<{ prompt: string }>(init).prompt : say('（带附件的消息）', '(a message with attachments)');
      return json(startTurn(id, prompt || '…'));
    }
    case 'interrupt':
      running.get(id)?.cancel();
      return json(thread(id));
    case 'settings':
      return json(update(id, body<Partial<ThreadDto>>(init)));
    case 'resume':
      return json(current);
    case 'shell':
      return method === 'POST' ? createShell(current, body<{ label?: string }>(init).label) : json(shellState(current));
    case 'automations':
      if (second && third === 'runs') return json({ runs: [] });
      return json({ automations: data().automations[id] ?? [] });
    case 'watches':
      return json({ watches: [] });
    case 'subagents': {
      const agents = data().subagents[id] ?? [];
      if (!second) return json({ agents, refreshing: false });
      const agent = agents.find((item) => item.id === second);
      return agent ? json({ agent, items: [], hasEarlierItems: false }) : fail(404, 'agent not found');
    }
    case 'export-turns': {
      const turns = data().turns[id] ?? [];
      return json({
        totalTurnCount: turns.length,
        turns: turns.map((turn, index) => ({ turnId: turn.id, turnNumber: index + 1, startedAt: turn.startedAt, status: turn.status, userPromptPreview: turn.items.find((item) => item.kind === 'userMessage')?.text.slice(0, 80) ?? '' })),
      });
    }
    case 'search': {
      const query = (url.searchParams.get('q') ?? '').toLowerCase();
      const matches = (data().turns[id] ?? []).flatMap((turn) => turn.items
        .filter((item) => (item.kind === 'userMessage' || item.kind === 'agentMessage') && query && item.text.toLowerCase().includes(query))
        .map((item) => ({ turnId: turn.id, itemId: item.id, role: item.kind === 'userMessage' ? 'user' : 'assistant', text: item.text, createdAt: item.createdAt ?? undefined })));
      return json({ matches, hasMore: false });
    }
    case 'pending-steers':
      return json({ pendingSteers: [] });
    default:
      break;
  }
  return unsupported();
}

// ---- Terminals -------------------------------------------------------------

const shells = new Map<string, TourShell>();

function shellState(current: ThreadDto) {
  const list = data().shells[current.id] ?? [];
  const active = list.find((shell) => shell.status !== 'exited') ?? null;
  return { threadId: current.id, workspaceId: current.workspaceId, workspacePathStatus: 'present', state: active ? active.status : 'not_created', shell: active, shells: list, activeShellId: active?.id ?? null };
}

function createShell(current: ThreadDto, label?: string) {
  const list = (data().shells[current.id] ??= []);
  const at = now();
  const root = workspace(current.workspaceId)?.absPath ?? SAMPLE_ROOT;
  const session: ShellSessionDto = {
    id: uid(), threadId: current.id, workspaceId: current.workspaceId, label: label ?? null, tmuxSessionName: 'tour', backend: 'pty',
    cwd: root, status: 'running', attachedViewerId: null, createdAt: at, updatedAt: at, lastActivityAt: null,
  };
  list.push(session);
  shells.set(session.id, new TourShell(root, () => data().files[current.workspaceId] ?? {}, () => listThreads(true)));
  return json({ ...shellState(current), shell: session, activeShellId: session.id });
}

function shellRoutes(id: string, action: string | undefined, init: RequestInit, method: string) {
  const session = Object.values(data().shells).flat().find((item) => item.id === id);
  if (!session) return fail(404, 'shell not found');
  if (method === 'PATCH') session.label = body<{ label?: string | null }>(init).label ?? null;
  if (action === 'terminate') {
    session.status = 'exited';
    shells.delete(id);
    broadcast({ type: 'shell.exited', shellId: id, timestamp: now(), payload: { threadId: session.threadId, state: 'exited' } });
  }
  return json(session);
}

// ---- Device ----------------------------------------------------------------

function models() {
  const efforts = ['low', 'medium', 'high'].map((reasoningEffort) => ({ reasoningEffort, description: reasoningEffort }));
  return [
    { id: 'gpt-5.4', model: 'gpt-5.4', displayName: 'GPT-5.4', description: say('样例模型', 'Sample model'), hidden: false, isDefault: true, selectionKind: 'model', defaultReasoningEffort: 'medium', supportedReasoningEfforts: efforts },
    { id: 'gpt-5.4-mini', model: 'gpt-5.4-mini', displayName: 'GPT-5.4 mini', description: say('更快的样例模型', 'A faster sample model'), hidden: false, isDefault: false, selectionKind: 'model', defaultReasoningEffort: 'low', supportedReasoningEfforts: efforts },
  ];
}

function capabilities() {
  return {
    branching: { fork: false, forkAt: false, hardRollback: false, resumeAt: false, rewindFiles: false },
    controls: { goals: false, liveModelSwitch: true, performanceMode: false, permissionRequests: false, planMode: false, sandboxMode: false },
    management: { hookTrust: false, hooks: false, hostConfigFiles: false, mcpStatus: false, models: true, providerSettings: false, skills: false },
    sessions: { close: false, delete: false, importLocal: true, list: true, load: true, read: true, resume: true },
    turns: { compact: false, interrupt: true, start: true, steer: false, streamInput: false },
    usage: { contextWindow: false, costUsd: false, tokenUsage: false },
  };
}

function plugins() {
  return [{
    id: 'remote-codex.terminal', name: 'Terminal', description: 'Per-thread PTY terminal.', version: 'tour', remoteCodex: '*', source: 'builtin',
    available: true, enabled: true, defaultEnabled: true,
    capabilities: { actions: [], artifactTypes: [], scopes: ['device', 'thread'], threadPanels: [{ id: 'terminal', kind: 'terminal', label: 'Terminal', artifactTypes: [] }], timelineRenderers: [] },
  }];
}

function backend(provider: 'codex' | 'claude') {
  return {
    provider, displayName: provider === 'codex' ? 'Codex' : 'Claude Code', description: say('样例 Harness', 'Sample harness'), enabled: true, isDefault: provider === 'codex',
    capabilities: capabilities(),
    installation: { busy: false, installCommand: null, installed: true, installedVersion: 'tour', lastError: null, latestVersion: null, packageName: provider, updateCommand: null },
    managementSchema: { buildRestart: false, configArchives: false, hookCommandTemplates: [], hostConfigFiles: [], mcpConfigFormat: 'none', providerConfigFormat: 'none', toolboxItems: [] },
    status: { lastError: null, lastStartedAt: now(), restartCount: 0, state: 'ready', transport: 'none' },
  };
}

function runtimeConfig() {
  return {
    appName: 'Pockymoe', appVersion: 'tour', environment: 'production', mode: 'local', hostMode: 'standalone', platform: 'linux', architecture: 'x86_64',
    host: 'localhost', port: 0, workspaceRoot: SAMPLE_ROOT,
    capabilities: { managedSignals: false, terminal: true, tmux: false, windowsTaskScheduler: false },
    hostCapabilities: { accountBinding: false, devicePeer: false, previews: false, relay: true },
  };
}

const DEVICE_NAME = () => say('样例设备', 'Sample device');

function device(): RelayDeviceDto {
  const at = now();
  return { id: TOUR_DEVICE_ID, ownerUserId: SAMPLE_USER.id, name: DEVICE_NAME(), token: null, tokenPreview: 'tour', connected: true, connectedAt: at, lastHeartbeatAt: at, createdAt: at };
}

function portal() {
  return {
    user: { ...SAMPLE_USER, role: 'user', enabled: true, createdAt: now() },
    devices: [device()],
    sharedWithMe: [], sharedByMe: data().shares, sharedDevicesWithMe: [], sharedThreadsWithMe: [], grantsByMe: data().grants,
  };
}

function relayRoutes(parts: string[], init: RequestInit, method: string) {
  const [kind, id] = parts;
  if (kind === 'portal') return json(portal());
  if (kind === 'access') return json({ kind: 'owner', shareId: null, grantId: null, scope: 'owner', threadAccess: 'control', workspaceAccess: 'write', workspaceId: null, canCreateThreads: true });
  if (kind === 'thread-links') return json([]);
  if (kind === 'presence') return json({ connected: true });
  if (kind === 'shares' || kind === 'grants') {
    const list = kind === 'shares' ? data().shares : data().grants;
    if (method === 'POST') {
      const input = body<Record<string, unknown>>(init);
      const at = now();
      const row = {
        ...input, id: uid(), ownerUserId: SAMPLE_USER.id, ownerUsername: SAMPLE_USER.username, targetUserId: `tour-${String(input.targetIdentifier)}`,
        targetUsername: String(input.targetIdentifier ?? ''), deviceName: DEVICE_NAME(), createdAt: at, revokedAt: null, expiresAt: null,
        lastAccessedAt: null, lastAccessedByUsername: null, accessEvents: [], deviceConnected: true, verifiedAt: at,
        ...(kind === 'grants' ? { threadId: null, threadTitle: null } : {}),
      };
      (list as unknown[]).push(row);
      return json(row);
    }
    const row = (list as Array<{ id: string }>).find((item) => item.id === id);
    if (!row) return fail(404, 'share not found');
    if (method === 'PATCH') Object.assign(row, body<Record<string, unknown>>(init));
    if (method === 'DELETE') {
      const index = (list as Array<{ id: string }>).indexOf(row);
      list.splice(index, 1);
      return json({ ...row, revokedAt: now() });
    }
    return json(row);
  }
  return unsupported();
}

function apiRoutes(parts: string[], url: URL, init: RequestInit, method: string): Response {
  const [first, second, third, ...rest] = parts;
  switch (first) {
    case 'config':
      if (second === 'runtime') return json(runtimeConfig());
      if (second === 'agent-isolation') return json(third === 'workspaces' ? workspace(rest[0] ?? '') : { deviceIsolated: false, peerAccess: false, workspaces: [] });
      return unsupported();
    case 'version':
      return json({ name: 'remote-codex', version: 'tour' });
    case 'plugins':
      return json(plugins());
    case 'storage':
      return json({ deviceName: DEVICE_NAME(), scan: null, job: null });
    case 'account':
      return json([]);
    case 'thread-activity':
      return json({ since: now(), hours: 24, threads: [] });
    case 'agent-runtimes':
      if (!second) return json([backend('codex'), backend('claude')]);
      if (third === 'models') return json(models());
      if (third === 'subscription-usage') return json({ usage: null, unavailable: true });
      if (third === 'status' || !third) return json(backend(second === 'claude' ? 'claude' : 'codex'));
      return json([]);
    case 'search': {
      const query = (url.searchParams.get('q') ?? '').toLowerCase();
      const matches = data().threads.filter((item) => query && item.title.toLowerCase().includes(query)).map((item) => {
        const space = workspace(item.workspaceId)!;
        return { threadId: item.id, threadTitle: item.title, workspaceId: space.id, workspaceLabel: space.label, workspacePath: space.absPath, turnId: null, itemId: null, kind: 'title', role: 'user', text: item.title, createdAt: item.createdAt };
      });
      return json({ matches, hasMore: false, nextOffset: null, scope: 'device' });
    }
    case 'workspaces': {
      if (!second) {
        if (method === 'POST') {
          const input = body<{ absPath?: string; label?: string; name?: string; source?: string; url?: string }>(init);
          const name = (input.label || input.name || input.absPath?.split('/').pop() || 'new-workspace').trim();
          const created: WorkspaceDto = { id: uid(), hostId: 'tour', label: name, absPath: input.absPath ?? `${SAMPLE_ROOT}/${name}`, isFavorite: false, createdAt: now(), lastOpenedAt: null };
          data().workspaces.push(created);
          data().files[created.id] = { 'README.md': `# ${name}\n` };
          return json(created);
        }
        return json(data().workspaces);
      }
      const space = workspace(second);
      if (!space) return fail(404, 'workspace not found');
      if (!third) {
        if (method === 'PATCH') Object.assign(space, body<Partial<WorkspaceDto>>(init));
        if (method === 'DELETE') {
          data().workspaces = data().workspaces.filter((item) => item.id !== second);
          return json(space);
        }
        return json(space);
      }
      if (third === 'favorite') {
        space.isFavorite = Boolean(body<{ isFavorite?: boolean; favorite?: boolean }>(init).isFavorite ?? body<{ favorite?: boolean }>(init).favorite);
        return json(space);
      }
      if (third === 'threads') {
        if (method === 'POST') return json(createThread({ ...body<{ title?: string }>(init), workspaceId: second }));
        return json(listThreads(url.searchParams.get('includeAgentThreads') === 'true', second));
      }
      if (third === 'files') return files(second, rest, url, init, method);
      if (third === 'search') return apiRoutes(['search'], url, init, method);
      return unsupported();
    }
    case 'threads': {
      if (!second) return json(listThreads(url.searchParams.get('includeAgentThreads') === 'true'));
      if (second === 'start' && method === 'POST') {
        const created = createThread(body(init));
        return created ? json(created) : fail(404, 'workspace not found');
      }
      if (second === 'import-candidates') return json(importCandidates());
      if (second === 'import' && method === 'POST') {
        const created = createThread({ workspaceId: DEMO_WORKSPACE, title: say('导入的会话', 'Imported session') });
        return detail(created!.id, url);
      }
      return threadRoutes(second, [third, ...rest].filter((part): part is string => part !== undefined), url, init, method);
    }
    case 'shells':
      return second ? shellRoutes(second, third, init, method) : unsupported();
    case 'transport':
      return json({});
    default:
      return unsupported();
  }
}

function importCandidates() {
  const at = new Date(Date.now() - 3 * 3_600_000).toISOString();
  return [{
    provider: 'codex', agentId: null, sessionId: '019a6c2e-tour-4b1d-sample', cwd: `${SAMPLE_ROOT}/pockymoe-demo`,
    title: say('在终端里和 Codex 聊过的会话', 'A session you had with Codex in a terminal'), preview: say('帮我看看为什么构建失败', 'Why does the build fail?'),
    createdAt: at, updatedAt: at, historyStatus: 'unknown',
  }];
}

/** Answers one request a tour page sent to the sample device. */
export async function handleTourRequest(path: string, init: RequestInit): Promise<Response> {
  const url = new URL(path, 'https://tour.invalid/');
  const method = (init.method ?? 'GET').toUpperCase();
  const parts = url.pathname.split('/').filter(Boolean).map(decodeURIComponent);
  // relay/devices/pockymoe-tour/api/... → api/...
  if (parts[0] === 'relay' && parts[1] === 'devices' && parts[2] === TOUR_DEVICE_ID) {
    if (parts[3] === 'api') return apiRoutes(parts.slice(4), url, init, method);
    return relayRoutes(parts.slice(3), init, method);
  }
  if (parts[0] === 'relay') return relayRoutes(parts.slice(1), init, method);
  return unsupported();
}

// ---- Socket ----------------------------------------------------------------

const sockets = new Set<(message: unknown) => void>();

function broadcast(message: unknown) {
  for (const send of sockets) send(message);
}

/** The sample device's `/ws`: thread events, and terminals attached through it. */
export function connectTourSocket(deliver: (data: string) => void) {
  const send = (message: unknown) => deliver(JSON.stringify(message));
  sockets.add(send);
  const viewers = new Map<string, () => void>();
  return {
    ready: () => send({ type: 'supervisor.connected', timestamp: now() }),
    send: (raw: string) => {
      let message: { type: string; shellId?: string; data?: string; timestamp?: string };
      try {
        message = JSON.parse(raw);
      } catch {
        return;
      }
      if (message.type === 'supervisor.ping') {
        send({ type: 'supervisor.pong', timestamp: now(), payload: { requestTimestamp: message.timestamp ?? null } });
        return;
      }
      const shellId = message.shellId ?? '';
      const shell = shells.get(shellId);
      const session = Object.values(data().shells).flat().find((item) => item.id === shellId);
      if (!shell || !session) {
        if (message.type === 'shell.attach') send({ type: 'shell.error', shellId, timestamp: now(), payload: { code: 'not_found', message: 'shell not found' } });
        return;
      }
      if (message.type === 'shell.attach') {
        const viewerId = uid();
        viewers.get(shellId)?.();
        viewers.set(shellId, shell.listen((data) => send({ type: 'shell.output', shellId, timestamp: now(), payload: { data } })));
        session.attachedViewerId = viewerId;
        send({ type: 'shell.connected', shellId, timestamp: now(), payload: { viewerId } });
        send({ type: 'shell.status', shellId, timestamp: now(), payload: { threadId: session.threadId, state: 'attached', viewerId } });
        send({ type: 'shell.output', shellId, timestamp: now(), payload: { data: shell.replay(), replace: true } });
      } else if (message.type === 'shell.input' && message.data) shell.input(message.data);
      else if (message.type === 'shell.clear') shell.clear();
      else if (message.type === 'shell.detach') {
        viewers.get(shellId)?.();
        viewers.delete(shellId);
      }
    },
    close: () => {
      sockets.delete(send);
      for (const stop of viewers.values()) stop();
    },
  };
}
