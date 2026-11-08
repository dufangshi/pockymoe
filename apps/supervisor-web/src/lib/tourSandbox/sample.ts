import type { AutomationDto } from '@pockymoe/shared';
import type {
  NativeSubagentDto,
  ThreadDto,
  ThreadHistoryItemDto,
  ThreadTurnDto,
  WorkspaceDto,
} from '@pockymoe/shared';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { TOUR_HELPER_THREAD, TOUR_MAIN_THREAD } from './tourDevice';

/** The tour's sample project: two workspaces, three conversations and their files. */

export const SAMPLE_ROOT = '/home/you/projects';
export const DEMO_WORKSPACE = 'b1f3c6d0-7a2e-4c51-9a10-5a3f0e2c7d01';
export const DOCS_WORKSPACE = 'b1f3c6d0-7a2e-4c51-9a10-5a3f0e2c7d02';
export const MAIN_THREAD = TOUR_MAIN_THREAD;
export const HELPER_THREAD = TOUR_HELPER_THREAD;
export const DOCS_THREAD = 'c27d9e41-5b8a-4f3c-8e61-2d4b7a9c1e03';
export const SAMPLE_USER = { id: 'tour-user', username: 'you', email: 'you@example.com' };

const zh = () => getLocale().toLowerCase().startsWith('zh');
/** Sample text in the reader's language. */
export const say = (chinese: string, english: string) => (zh() ? chinese : english);

const minutes = (count: number) => new Date(Date.now() - count * 60_000).toISOString();

export function sampleWorkspaces(): WorkspaceDto[] {
  return [
    { id: DEMO_WORKSPACE, hostId: 'tour', label: 'pockymoe-demo', absPath: `${SAMPLE_ROOT}/pockymoe-demo`, isFavorite: true, createdAt: minutes(60 * 24 * 3), lastOpenedAt: minutes(4) },
    { id: DOCS_WORKSPACE, hostId: 'tour', label: 'docs-site', absPath: `${SAMPLE_ROOT}/docs-site`, isFavorite: false, createdAt: minutes(60 * 24 * 2), lastOpenedAt: minutes(90) },
  ];
}

export function sampleFiles(): Record<string, Record<string, string>> {
  return {
    [DEMO_WORKSPACE]: {
      'README.md': '# pockymoe-demo\n\nA small sample project for the Pockymoe tour.\n\n- `src/auth/login.ts`: sign-in with retries\n- `src/app.ts`: the entry point\n- `tests/login.test.ts`: written by a helper agent\n',
      'package.json': '{\n  "name": "pockymoe-demo",\n  "private": true,\n  "scripts": {\n    "test": "vitest run"\n  }\n}\n',
      'src/app.ts': "import { login } from './auth/login';\n\nexport async function main() {\n  const session = await login({ user: 'demo', password: 'demo' });\n  console.log(`Signed in as ${session.user}`);\n}\n",
      'src/auth/login.ts': "import { request } from '../http';\n\nconst RETRIES = 2;\n\n/** Signs in, retrying once on a server error. */\nexport async function login(input: { user: string; password: string }) {\n  for (let attempt = 0; ; attempt += 1) {\n    const response = await request('/session', { method: 'POST', body: input });\n    if (response.ok) return response.json() as Promise<{ user: string; token: string }>;\n    if (response.status < 500 || attempt >= RETRIES) throw new Error(`login failed: ${response.status}`);\n  }\n}\n",
      'src/http.ts': "export function request(path: string, init: { method: string; body?: unknown }) {\n  return fetch(path, { method: init.method, body: JSON.stringify(init.body) });\n}\n",
      'tests/login.test.ts': "import { describe, expect, it, vi } from 'vitest';\nimport { login } from '../src/auth/login';\n\ndescribe('login', () => {\n  it('retries a server error', async () => {\n    const fetch = vi.fn()\n      .mockResolvedValueOnce({ ok: false, status: 500 })\n      .mockResolvedValueOnce({ ok: true, json: async () => ({ user: 'demo', token: 't' }) });\n    vi.stubGlobal('fetch', fetch);\n    await expect(login({ user: 'demo', password: 'demo' })).resolves.toMatchObject({ user: 'demo' });\n    expect(fetch).toHaveBeenCalledTimes(2);\n  });\n});\n",
      'docs/notes.md': '# Notes\n\nEdit this file in the tour: it lives only in this browser tab.\n',
    },
    [DOCS_WORKSPACE]: {
      'index.md': '# Docs\n\nWelcome to the sample documentation site.\n',
      'guide/getting-started.md': '# Getting started\n\n1. Add a device.\n2. Add a workspace.\n3. Start a conversation.\n',
    },
  };
}

function thread(input: Partial<ThreadDto> & Pick<ThreadDto, 'id' | 'workspaceId' | 'title'>): ThreadDto {
  return {
    provider: 'codex',
    providerSessionId: null,
    source: 'supervisor',
    model: 'gpt-5.4',
    reasoningEffort: 'medium',
    collaborationMode: 'default',
    approvalMode: 'yolo',
    status: 'idle',
    summaryText: null,
    lastError: null,
    activeTurnId: null,
    isLoaded: true,
    isPinned: false,
    createdAt: minutes(60 * 5),
    updatedAt: minutes(5),
    lastTurnStartedAt: minutes(12),
    lastTurnCompletedAt: minutes(5),
    lineageDepth: 0,
    ...input,
  } as ThreadDto;
}

export function sampleThreads(): ThreadDto[] {
  return [
    thread({ id: MAIN_THREAD, workspaceId: DEMO_WORKSPACE, title: say('重构登录流程', 'Refactor sign-in'), descendantCount: 1 } as Partial<ThreadDto> & Pick<ThreadDto, 'id' | 'workspaceId' | 'title'>),
    thread({
      id: HELPER_THREAD, workspaceId: DEMO_WORKSPACE, title: say('补充单元测试', 'Add unit tests'),
      provider: 'claude', model: 'claude-sonnet-5-5', parentThreadId: MAIN_THREAD, rootThreadId: MAIN_THREAD,
      lineageDepth: 1, agentName: 'tester', createdAt: minutes(11), lastTurnStartedAt: minutes(11), lastTurnCompletedAt: minutes(7), updatedAt: minutes(7),
    } as Partial<ThreadDto> & Pick<ThreadDto, 'id' | 'workspaceId' | 'title'>),
    thread({ id: DOCS_THREAD, workspaceId: DOCS_WORKSPACE, title: say('更新文档首页', 'Update the docs home page'), createdAt: minutes(60 * 30), lastTurnStartedAt: minutes(95), lastTurnCompletedAt: minutes(90), updatedAt: minutes(90) }),
  ];
}

const output = (command: string, text: string) =>
  `Tool: exec_command\n\nKind: execute\n\nStatus: completed\n\nInput:\n${JSON.stringify({ cmd: command })}\n\nOutput:\n${JSON.stringify(text)}`;

function command(id: string, cmd: string, out: string, at: string): ThreadHistoryItemDto {
  return { id, kind: 'commandExecution', status: 'completed', text: cmd, detailText: output(cmd, out), createdAt: at };
}

const LOGIN_DIFF = `--- a/src/auth/login.ts
+++ b/src/auth/login.ts
@@ -1,9 +1,12 @@
 import { request } from '../http';

+const RETRIES = 2;
+
-/** Signs in. */
+/** Signs in, retrying once on a server error. */
 export async function login(input: { user: string; password: string }) {
-  const response = await request('/session', { method: 'POST', body: input });
-  if (!response.ok) throw new Error('login failed');
-  return response.json();
+  for (let attempt = 0; ; attempt += 1) {
+    const response = await request('/session', { method: 'POST', body: input });
+    if (response.ok) return response.json() as Promise<{ user: string; token: string }>;
+    if (response.status < 500 || attempt >= RETRIES) throw new Error(\`login failed: \${response.status}\`);
+  }
 }`;

const WAKE_HEADER = '[Pockymoe wake: the delegates you registered a wake for have settled]\n';

function helperReport() {
  return say(
    '已在 tests/login.test.ts 补充 2 个用例：服务端 500 会重试一次；401 不重试直接报错。vitest 全部通过（3 passed）。',
    'Added two cases to tests/login.test.ts: a 500 is retried once, a 401 fails without retrying. vitest passes (3 passed).',
  );
}

export function sampleTurns(): Record<string, ThreadTurnDto[]> {
  const ask = say(
    '登录接口偶尔返回 500。帮我重构 src/auth/login.ts，让它遇到服务端错误时重试；再开一个 Agent 去补单元测试，测完告诉我结果。',
    'Sign-in sometimes returns a 500. Refactor src/auth/login.ts so it retries server errors, and start another agent to add unit tests; tell me how they go.',
  );
  const taskText = say('为 src/auth/login.ts 的重试逻辑补单元测试：500 重试一次，4xx 不重试。完成后汇报。', 'Add unit tests for the retry in src/auth/login.ts: retry a 500 once, never a 4xx. Report when done.');
  const create = `pockymoe thread create --provider claude --title "${say('补充单元测试', 'Add unit tests')}" --text-file task.md --wake`;
  const delegated = `{"threadId":"${HELPER_THREAD}","title":"${say('补充单元测试', 'Add unit tests')}","status":"running","wake":"registered"}`;
  const task: ThreadTurnDto = {
    id: 'tour-turn-task', status: 'completed', startedAt: minutes(13), completedAt: minutes(10), error: null, model: 'gpt-5.4',
    items: [
      { id: 'tour-turn-task:user', kind: 'userMessage', origin: 'user', text: ask, createdAt: minutes(13) },
      { id: 'tour-turn-task:plan', kind: 'agentMessage', responsePhase: 'commentary', status: 'completed', text: say('先看现在的登录实现。', 'First, the current sign-in code.'), createdAt: minutes(13) },
      command('tour-turn-task:rg', 'rg -n "login" src', 'src/app.ts:1:import { login } from \'./auth/login\';\nsrc/auth/login.ts:4:export async function login(', minutes(13)),
      { id: 'tour-turn-task:edit', kind: 'fileChange', status: 'completed', text: 'src/auth/login.ts', previewText: 'src/auth/login.ts', changedFiles: 1, addedLines: 7, removedLines: 4, detailText: LOGIN_DIFF, createdAt: minutes(12) },
      command('tour-turn-task:delegate', create, delegated, minutes(11)),
      { id: 'tour-turn-task:handoff', kind: 'agentMessage', responsePhase: 'commentary', status: 'completed', text: say('已把测试交给「补充单元测试」线程，它结束时会唤醒我。', 'Handed the tests to the "Add unit tests" thread; it wakes me when it settles.'), createdAt: minutes(11) },
    ],
  };
  const report = helperReport();
  const envelope = `${WAKE_HEADER}- tester (${HELPER_THREAD}): idle\n  closing message: ${report}\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1`;
  const wake: ThreadTurnDto = {
    id: 'tour-turn-wake', status: 'completed', startedAt: minutes(7), completedAt: minutes(5), error: null, model: 'gpt-5.4',
    items: [
      { id: 'tour-turn-wake:user', kind: 'userMessage', origin: 'peer', text: envelope, createdAt: minutes(7),
        peerWork: { taskTurnId: task.id, messages: [{ id: 'tour-wake:tester', name: 'tester', fromThreadId: HELPER_THREAD, kind: 'result', subject: 'idle', text: report }] } },
      { id: 'tour-turn-wake:check', kind: 'agentMessage', responsePhase: 'commentary', status: 'completed', text: say('先自己跑一遍测试核实报告。', 'Checking the report by running the tests myself.'), createdAt: minutes(6) },
      command('tour-turn-wake:test', 'npm test', ' ✓ tests/login.test.ts (3 tests) 12ms\n\n Test Files  1 passed (1)\n      Tests  3 passed (3)', minutes(6)),
      { id: 'tour-turn-wake:final', kind: 'agentMessage', responsePhase: 'final', status: 'completed', createdAt: minutes(5), text: say(
        '完成了：\n\n- **重构**：`src/auth/login.ts` 遇到 5xx 最多重试 2 次，4xx 直接报错。\n- **测试**：「补充单元测试」线程加了 2 个用例，我核实过，`npm test` 3 项全部通过。\n\n下次可以直接让我“开一个 Agent 去做 X”，它们之间会通过 `pockymoe thread send` 互相发消息。',
        'Done:\n\n- **Refactor**: `src/auth/login.ts` retries a 5xx up to twice and fails a 4xx at once.\n- **Tests**: the "Add unit tests" thread added two cases; I checked them, and `npm test` passes all 3.\n\nNext time just ask me to "start an agent for X"; agents message each other with `pockymoe thread send`.',
      ) },
    ],
  };
  const helper: ThreadTurnDto = {
    id: 'tour-turn-helper', status: 'completed', startedAt: minutes(11), completedAt: minutes(7), error: null, model: 'claude-sonnet-5-5',
    items: [
      { id: 'tour-turn-helper:user', kind: 'userMessage', origin: 'peer', createdAt: minutes(11),
        text: `[Pockymoe task from thread ${MAIN_THREAD} | ${say('补充登录单元测试', 'Add sign-in unit tests')}]\n${taskText}` },
      command('tour-turn-helper:read', 'cat src/auth/login.ts', 'export async function login(input) { … }', minutes(10)),
      { id: 'tour-turn-helper:edit', kind: 'fileChange', status: 'completed', text: 'tests/login.test.ts', previewText: 'tests/login.test.ts', changedFiles: 1, addedLines: 14, removedLines: 0,
        detailText: '--- /dev/null\n+++ b/tests/login.test.ts\n@@ -0,0 +1,14 @@\n+import { describe, expect, it, vi } from \'vitest\';\n+import { login } from \'../src/auth/login\';\n+\n+describe(\'login\', () => {\n+  it(\'retries a server error\', async () => {\n+    …\n+  });\n+});', createdAt: minutes(9) },
      command('tour-turn-helper:test', 'npx vitest run', ' Test Files  1 passed (1)\n      Tests  3 passed (3)', minutes(8)),
      { id: 'tour-turn-helper:final', kind: 'agentMessage', responsePhase: 'final', status: 'completed', text: report, createdAt: minutes(7) },
    ],
  };
  const docs: ThreadTurnDto = {
    id: 'tour-turn-docs', status: 'completed', startedAt: minutes(95), completedAt: minutes(90), error: null, model: 'gpt-5.4',
    items: [
      { id: 'tour-turn-docs:user', kind: 'userMessage', origin: 'user', text: say('首页加一段“快速开始”。', 'Add a "Getting started" section to the home page.'), createdAt: minutes(95) },
      { id: 'tour-turn-docs:edit', kind: 'fileChange', status: 'completed', text: 'guide/getting-started.md', previewText: 'guide/getting-started.md', changedFiles: 1, addedLines: 5, removedLines: 0, detailText: '--- /dev/null\n+++ b/guide/getting-started.md\n@@ -0,0 +1,5 @@\n+# Getting started\n+\n+1. Add a device.\n+2. Add a workspace.\n+3. Start a conversation.', createdAt: minutes(92) },
      { id: 'tour-turn-docs:final', kind: 'agentMessage', responsePhase: 'final', status: 'completed', text: say('已新增 guide/getting-started.md，并在首页链接过去。', 'Added guide/getting-started.md and linked it from the home page.'), createdAt: minutes(90) },
    ],
  };
  return { [MAIN_THREAD]: [task, wake], [HELPER_THREAD]: [helper], [DOCS_THREAD]: [docs] };
}

export function sampleAutomations(): Record<string, AutomationDto[]> {
  return {
    [MAIN_THREAD]: [{
      id: 'tour-automation-nightly', threadId: MAIN_THREAD, sourceKind: 'supervisor',
      definition: { name: say('每晚跑测试', 'Nightly tests'), trigger: { kind: 'interval', everySeconds: 86_400 }, action: { kind: 'prompt', text: say('跑一遍 npm test，失败就修。', 'Run npm test and fix any failure.') } },
      state: 'enabled', nextRunAt: new Date(Date.now() + 6 * 3_600_000).toISOString(), createdAt: minutes(60 * 24), updatedAt: minutes(60 * 24), pendingCount: 0, missedCount: 0, error: null,
    }],
  };
}

export function sampleSubagents(): Record<string, NativeSubagentDto[]> {
  return {
    [MAIN_THREAD]: [{
      id: 'tour-subagent-explore', name: 'explore', status: 'completed', startedAt: minutes(13), completedAt: minutes(12), parentToolCallId: null,
      provider: 'codex', nativeSessionId: null, model: 'gpt-5.4-mini', prompt: say('找出所有调用 login() 的地方。', 'Find every caller of login().'),
      updatedAt: minutes(12), latestActivity: say('找到 1 处调用：src/app.ts', 'Found one caller: src/app.ts'), tokenUsage: null, priceEstimate: null, activityCount: 3, detailsAvailable: false,
    }],
  };
}

/** What the sample agent answers a new prompt with: progress, one command, a reply. */
export function scriptedReply(prompt: string) {
  return {
    commentary: say('收到，我先看一下工作区。', 'On it; looking at the workspace first.'),
    command: 'ls src',
    commandOutput: 'app.ts  auth  http.ts',
    answer: say(
      `这是教程里的样例 Agent，回复是预先写好的，不会调用真实模型。\n\n你刚才说：「${prompt.slice(0, 80)}」。在你自己的设备上，Agent 会在这个工作区里读写文件、运行命令，并像这样边做边汇报。`,
      `This is the tour's sample agent: its replies are scripted and no real model runs.\n\nYou said: "${prompt.slice(0, 80)}". On your own device, an agent reads and writes this workspace, runs commands and reports like this as it works.`,
    ),
  };
}
