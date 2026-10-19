import { randomUUID } from 'node:crypto';
import { mkdir, writeFile, readFile } from 'node:fs/promises';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { expect, test, type Page, type APIRequestContext } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  // This spec exercises the classic phone bars/tabs and composer split controls.
  await page.addInitScript(() => {
    localStorage.setItem('pockymoe.phoneDock', 'off');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [] }));
  });
});

test.use({ actionTimeout: 15_000 });

const base = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
const screenshotRoot = process.env.WORKBENCH_SCREENSHOT_DIR;
// The split trigger lives in the thread tools menu beside the tabs; open it when hidden.
async function revealSplitTrigger(page: Page) {
  const trigger = page.getByTestId('workbench-split-trigger');
  if (!(await trigger.isVisible())) await page.locator('.matter-tabs-row .matter-toolbar-toggle').click();
  await expect(trigger).toBeVisible();
  return trigger;
}
// One split trigger opens the picker in every arrangement.
async function openSplitPicker(page: Page) {
  const trigger = page.getByTestId('workbench-split-trigger');
  if ((await trigger.getAttribute('aria-expanded')) !== 'true') await (await revealSplitTrigger(page)).click();
}

async function selectSplit(page: Page, id: string) {
  await openSplitPicker(page);
  await page.getByTestId('workbench-thread-picker').locator(`[data-thread-id="${id}"]`).click();
}

// Phone splits switch the two conversations beside the composer's timeline capsule.
const paneSwitch = (page: Page, side: 'primary' | 'reference') =>
  page.locator(`.thread-pane-switch[data-side="${side}"]:visible`);


// Only the isolated fake Supervisor database is edited. All UI data is then read
// through the normal API. Native-agent metadata is an explicitly labelled fixture.
async function fixture(request: APIRequestContext) {
  const absPath = path.resolve(
    process.env.E2E_WORKSPACE_ROOT!,
    `workbench-${randomUUID()}`,
  );
  await mkdir(absPath, { recursive: true });
  await writeFile(
    path.join(absPath, '验收记录.md'),
    '# 工作台验收\n\n双会话保持独立；两边可同时发送消息。\n',
  );
  const wsResponse = await request.post(`${base}/api/workspaces`, {
    data: { absPath, label: '远程工作台 · 布局评审' },
  });
  expect(wsResponse.ok()).toBeTruthy();
  const ws = await wsResponse.json();
  const create = async (title: string, parentThreadId?: string) => {
    const response = await request.post(`${base}/api/threads/start`, {
      data: {
        workspaceId: ws.id,
        title,
        provider: 'codex',
        model: 'default',
        approvalMode: 'yolo',
        parentThreadId,
      },
    });
    expect(response.ok()).toBeTruthy();
    const value = await response.json();
    return value.thread ?? value;
  };
  const a = await create('实现：双会话工作台');
  const b = await create('评审：发送目标与手机布局', a.id);
  const c = await create('验证：恢复与异常状态', a.id);
  for (const thread of [a, b, c]) {
    expect(
      (
        await request.post(`${base}/api/threads/${thread.id}/prompt`, {
          data: { prompt: 'hello' },
        })
      ).ok(),
    ).toBeTruthy();
    await expect
      .poll(
        async () =>
          (await (await request.get(`${base}/api/threads/${thread.id}`)).json())
            .thread.status,
      )
      .toBe('idle');
  }
  const filename = path.resolve(process.env.E2E_DATABASE_URL!);
  // Both the standalone workbench runner and focused task runners use isolated data.
  expect(filename).toMatch(/(?:\.temp\/workbench\/|\.scratch\/[^/]+\/e2e\/)/);
  const db = new DatabaseSync(filename);
  db.exec('PRAGMA busy_timeout=5000');
  db.function('search_fold', { deterministic: true }, (value) =>
    typeof value === 'string' ? value.toLowerCase() : value,
  );
  db.function('search_body', { deterministic: true }, (text, kind, _source) =>
    ['userMessage', 'agentMessage'].includes(String(kind)) ? text : null,
  );
  const text = [
    [
      '请实现双会话工作台，设备、工作区和发送目标要始终清楚。',
      '## 工作台实现已准备\n\n左侧负责实现，右侧可以独立回复评审意见。\n\n### 核心约束\n\n- **独立发送目标**：两边输入各自绑定当前会话。\n- **独立阅读**：两边历史与滚动互不干扰。\n- **安全切换**：草稿与附件随会话保留，不会串线。\n\n### 待评审\n\n请检查手机的主 / 参考切换，以及刷新后的布局恢复。\n\n> 本页为 fake harness 验收示例；不连接真实模型。',
    ],
    [
      '请独立评审双会话布局，并给出可操作的验收意见。',
      '## 独立评审结论\n\n布局能同时呈现实现内容与评审证据。\n\n### 已检查\n\n1. 两个会话各有自己的输入框。\n2. 关闭参考视图不停止后台线程。\n3. 主 / 参考会话的草稿不会串线。\n\n### 建议\n\n在手机上用会话标题切换，每个视图都能回复自己的会话。\n\n**结果：可以进入视觉评审。**\n\n> API / native 状态均为隔离验收 fixture。',
    ],
    ['验证损坏布局的恢复路径。', '验证发现一处异常配置；请检查恢复策略。'],
  ];
  try {
    for (const [index, thread] of [a, b, c].entries()) {
      db.prepare(
        'UPDATE thread_turns SET display_prompt=? WHERE thread_id=?',
      ).run(text[index]![0], thread.id);
      const rows = db
        .prepare(
          'SELECT id,item_json FROM thread_history_items WHERE thread_id=?',
        )
        .all(thread.id) as { id: string; item_json: string }[];
      for (const row of rows) {
        const item = JSON.parse(row.item_json);
        if (item.kind !== 'userMessage' && item.kind !== 'agentMessage')
          continue;
        item.text = text[index]![item.kind === 'userMessage' ? 0 : 1];
        db.prepare(
          'UPDATE thread_history_items SET item_json=? WHERE id=?',
        ).run(JSON.stringify(item), row.id);
      }
    }
    const nativeTurn = db
      .prepare('SELECT id FROM thread_turns WHERE thread_id=?')
      .get(a.id) as { id: string };
    for (const [id, name, status] of [
      ['native-check-layout', '原生代理 · 检查布局', 'completed'],
      ['native-check-width', '原生代理 · 宽度回归', 'failed'],
    ]) {
      const timestamp = new Date().toISOString();
      const item = {
        id,
        kind: 'agentToolCall',
        text: name,
        status,
        createdAt: timestamp,
        detailText: '隔离 fake harness fixture：已记录的原生工具结果。',
      };
      db.prepare(
        'INSERT INTO thread_history_items(id,thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES (?,?,?,?,?,?,?)',
      ).run(
        randomUUID(),
        a.id,
        nativeTurn.id,
        id,
        JSON.stringify(item),
        timestamp,
        timestamp,
      );
    }
    db.prepare(
      "UPDATE threads SET status='failed',last_error=? WHERE id=?",
    ).run('Fixture：布局配置损坏，已回退默认比例。', c.id);
  } finally {
    db.close();
  }
  return { a, b, c };
}

test('dual conversations send concurrently to real thread IDs with independent drafts and recoverable views', async ({ page, request }, testInfo) => {
  const mobile = testInfo.project.name === 'mobile-chromium';
  await page.setViewportSize(mobile ? { width: 390, height: 844 } : { width: 1440, height: 1000 });
  await page.addInitScript((preset) => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem('remote-codex-theme-mode', 'light');
    if (preset) localStorage.setItem('pockymoe-theme-preset', preset);
    // The onboarding welcome card would cover the workbench.
    localStorage.setItem('pockymoe.onboarding.v1:'+JSON.stringify([location.origin,'local:owner']),JSON.stringify({welcomeDismissed:true,completed:[],resume:{}}));
  }, process.env.WORKBENCH_THEME_PRESET);
  const { a, b, c } = await fixture(request);
  await page.goto(`/threads/${a.id}`);
  const primary = page.getByTestId('primary-pane');
  const reference = page.getByTestId('reference-pane');
  const left = primary.getByRole('textbox', { name: '提示词', exact: true });
  const right = reference.getByRole('textbox', { name: '提示词', exact: true });
  const showLeft = async () => { if (mobile) await paneSwitch(page, 'primary').click(); };
  const showRight = async () => { if (mobile) await paneSwitch(page, 'reference').click(); };
  await expect(left).toBeVisible();
  await expect(primary.locator('.workbench-pane-heading')).toHaveCount(0);
  await expect(page.getByText('发送目标 · 主会话', { exact: true })).toHaveCount(0);
  await left.fill('左侧独立草稿');
  // One selection in the thread tools menu opens the second conversation.
  await revealSplitTrigger(page);
  await selectSplit(page, b.id);
  await expect(right).toBeVisible();
  await expect(reference).toContainText('独立评审结论');
  await right.fill('右侧独立草稿');
  await expect(page.getByTestId('chat-composer')).toHaveCount(2);
  await selectSplit(page, c.id);
  await expect(reference.getByRole('textbox', { name: '提示词', exact: true })).toHaveText('');
  await selectSplit(page, b.id);
  await expect(right).toHaveText('右侧独立草稿');
  await showLeft();
  await expect(left).toHaveText('左侧独立草稿');
  const snap = async (name: string) => {
    if (!screenshotRoot) return;
    await mkdir(screenshotRoot, { recursive: true });
    await page.screenshot({ path: path.join(screenshotRoot, name), scale: 'css' });
  };
  if (mobile) {
    await snap('mobile-main.png');
    await showRight();
    await expect(right).toBeVisible();
    await snap('mobile-reference.png');
    await page.goBack();
    await expect(left).toBeVisible();
    await expect(page).toHaveURL(new RegExp(`/threads/${a.id}$`));
  } else {
    await snap('desktop-compare.png');
    const separator = page.getByRole('separator', { name: '调整对照比例' });
    await separator.focus(); await separator.press('ArrowLeft');
    await expect(separator).toHaveAttribute('aria-valuenow', '50');
  }
  // Hold both HTTP submissions: the second must reach its own endpoint while the first is in flight.
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const sent: string[] = [];
  await page.route('**/api/threads/*/prompt', async route => {
    sent.push(route.request().url().match(/threads\/([^/]+)\/prompt/)![1]!);
    await held;
    await route.continue();
  });
  await showLeft();
  await primary.getByRole('button', { name: '发送提示词', exact: true }).click();
  await expect.poll(() => sent).toEqual([a.id]);
  await showRight();
  await reference.locator('input[type=file]').last().setInputFiles({ name: 'review.txt', mimeType: 'text/plain', buffer: Buffer.from('right attachment only') });
  await reference.getByRole('button', { name: '发送提示词', exact: true }).click();
  await expect.poll(() => sent).toEqual([a.id, b.id]);
  release();
  await expect(right).toHaveText('');
  await showLeft();
  await expect(left).toHaveText('');
  const get = async (id: string) => (await (await request.get(`${base}/api/threads/${id}`)).json());
  await expect.poll(async () => (await get(a.id)).turns.length).toBe(2);
  await expect.poll(async () => (await get(b.id)).turns.length).toBe(2);
  const aTurn = (await get(a.id)).turns.at(-1).items.find((item: { kind: string }) => item.kind === 'userMessage');
  const bTurn = (await get(b.id)).turns.at(-1).items.find((item: { kind: string }) => item.kind === 'userMessage');
  expect(aTurn.text).toContain('左侧独立草稿');
  expect(bTurn.text).toContain('右侧独立草稿');
  const uploadedPath = bTurn.text.match(/\[FILE ([^\]]+)\]/)![1]!;
  expect(uploadedPath).toContain(`/threads/${b.id}/review-`);
  expect(await readFile(path.resolve((await get(b.id)).workspace.absPath, uploadedPath), 'utf8')).toBe('right attachment only');
  expect(aTurn.text).not.toContain('[FILE');
  await page.unroute('**/api/threads/*/prompt');
  await left.fill('切换后仍属于左侧');
  await showRight();
  await right.fill('切换后仍属于右侧');
  if (mobile) {
    // Phones have no pane header: the pane switch shows each side, the lit trigger closes the split.
    await expect(page.getByTestId('make-primary')).toHaveCount(0);
    await expect(page.locator('.workbench-mobile-views')).toHaveCount(0);
    await expect(paneSwitch(page, 'reference')).toHaveAttribute('aria-pressed', 'true');
  } else {
    await page.getByTestId('make-primary').click();
    await expect(page).toHaveURL(new RegExp(`/threads/${b.id}$`));
    await expect(primary.getByRole('textbox', { name: '提示词', exact: true })).toHaveText('切换后仍属于右侧');
    await expect(reference.getByRole('textbox', { name: '提示词', exact: true })).toHaveText('切换后仍属于左侧');
    await page.getByTestId('make-primary').click();
    await expect(page).toHaveURL(new RegExp(`/threads/${a.id}$`));
  }
  await showRight();
  const trigger = page.getByTestId('workbench-split-trigger');
  await expect(trigger).toHaveAttribute('aria-pressed', 'true');
  const toolbarCount = await page.locator('.matter-tabs-row > button, .matter-tabs-row > span > button').count();
  await (await revealSplitTrigger(page)).click();
  await expect(page.getByTestId('workbench-thread-picker')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('split-menu.png'), scale: 'css', animations: 'disabled' });
  await page.getByTestId('workbench-thread-picker').getByRole('button', { name: '关闭分屏', exact: true }).click();
  await expect(reference).not.toBeVisible();
  expect(await page.locator('.matter-tabs-row > button, .matter-tabs-row > span > button').count()).toBe(toolbarCount);
  await trigger.click();
  await page.getByTestId('workbench-thread-picker').getByRole('button', { name: '恢复分屏', exact: true }).click();
  await showRight();
  await expect(reference).toBeVisible();
  expect(await page.locator('.matter-tabs-row > button, .matter-tabs-row > span > button').count()).toBe(toolbarCount);
  await trigger.click();
  await page.getByTestId('workbench-thread-picker').getByRole('button', { name: '关闭分屏', exact: true }).click();
  await expect(trigger).toHaveAttribute('aria-pressed', 'false');
  await selectSplit(page, b.id);
  await expect(right).toHaveText('切换后仍属于右侧');
  await openSplitPicker(page);
  await page.getByRole('button', { name: '协作进度', exact: true }).click();
  await expect(page.getByTestId('managed-summary')).toHaveCount(2);
  await expect(page.getByTestId('native-summary')).toHaveCount(2);
  if (!mobile) await snap('desktop-collaboration.png');
  await selectSplit(page, b.id);
  await page.evaluate(() => {
    for (const key of Object.keys(localStorage))
      if (key.includes('presentation.v1:') && key.endsWith('.arrangement')) localStorage.setItem(key, '{broken');
  });
  await page.reload();
  await expect(page.getByTestId('workbench-panels')).toHaveAttribute('data-mode', 'thread');
  await showRight();
  await expect(right).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(mobile ? 390 : 1440);
  await page.unrouteAll({ behavior: 'wait' });
});

test('opening and closing references preserves a running primary and ignores late reference responses', async ({
  page,
  request,
}) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const { a, b, c } = await fixture(request);
  let releaseB!: () => void;
  const delayed = new Promise<void>((resolve) => {
    releaseB = resolve;
  });
  let bRequested!: () => void;
  const requested = new Promise<void>((resolve) => {
    bRequested = resolve;
  });
  await page.route(`**/api/threads/${b.id}?*`, async (route) => {
    const response = await route.fetch();
    bRequested();
    await delayed;
    await route.fulfill({ response });
  });
  await page.route(`**/api/threads/${c.id}?*`, async (route) => {
    const response = await route.fetch();
    const detail = await response.json();
    for (const turn of detail.turns)
      for (const item of turn.items)
        if (item.kind === 'agentMessage')
          item.text =
            '参考区长历史\n\n' +
            Array.from(
              { length: 60 },
              (_, index) => `第 ${index + 1} 项：独立滚动和请求来源隔离。`,
            ).join('\n\n');
    await route.fulfill({ response, json: detail });
  });
  let socketCount = 0;
  page.on('websocket', () => {
    socketCount += 1;
  });
  await page.goto(`/threads/${a.id}`);
  const primary = page.getByTestId('primary-pane');
  const reference = page.getByTestId('reference-pane');
  await expect(primary.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
  expect(
    (
      await request.post(`${base}/api/threads/${a.id}/prompt`, {
        data: { prompt: 'inspect this repository' },
      })
    ).ok(),
  ).toBeTruthy();
  await expect(primary.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  const baselineSocketCount = socketCount;
  await selectSplit(page, b.id);
  await requested;
  await expect(primary.getByRole('textbox', { name: 'Prompt', exact: true })).toBeVisible();
  await selectSplit(page, c.id);
  await expect(reference).toContainText(c.title);
  releaseB();
  await expect(reference).toContainText('参考区长历史');
  await expect(reference.locator('.workbench-pane-heading')).not.toContainText(
    b.title,
  );
  const primaryScroll = primary.locator('.thread-graph-scroll-container');
  const referenceScroll = reference.locator('.thread-graph-scroll-container');
  const primaryOffset = await primaryScroll.evaluate((node) => node.scrollTop);
  await referenceScroll.evaluate((node) => {
    node.scrollTop = 200;
    node.dispatchEvent(new Event('scroll'));
  });
  await expect
    .poll(() => referenceScroll.evaluate((node) => node.scrollTop))
    .toBeGreaterThan(100);
  expect(await primaryScroll.evaluate((node) => node.scrollTop)).toBe(
    primaryOffset,
  );
  expect(socketCount).toBe(baselineSocketCount);
  await reference
    .getByRole('button', { name: 'Close split', exact: true })
    .click();
  await expect(reference).not.toBeVisible();
  expect(
    (await (await request.get(`${base}/api/threads/${a.id}`)).json()).thread
      .status,
  ).toBe('running');
  await expect(
    primary.getByText('ok: inspect this repository', { exact: true }),
  ).toBeVisible({ timeout: 35000 });
  await expect
    .poll(
      async () =>
        (await (await request.get(`${base}/api/threads/${a.id}`)).json()).thread
          .status,
    )
    .toBe('idle');
  await page.unrouteAll({ behavior: 'wait' });
});

test('secondary queue, steer, stop and failed sends stay bound to their conversation', async ({ page, request }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const { a, b } = await fixture(request);
  await page.goto(`/threads/${a.id}`);
  await selectSplit(page, b.id);
  const primary = page.getByTestId('primary-pane');
  const secondary = page.getByTestId('reference-pane');
  const left = primary.getByRole('textbox', { name: 'Prompt', exact: true });
  const right = secondary.getByRole('textbox', { name: 'Prompt', exact: true });
  await expect(right).toBeVisible();
  const requests: string[] = [];
  page.on('request', request => { if (request.method() === 'POST' || request.method() === 'DELETE') requests.push(new URL(request.url()).pathname); });
  await left.fill('inspect this repository left');
  await primary.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await right.fill('inspect this repository right');
  await secondary.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  const stop = secondary.getByRole('button', { name: 'Stop Current Turn', exact: true });
  await expect(stop).toBeVisible();
  await expect(primary.getByRole('button', { name: 'Stop Current Turn', exact: true })).toBeVisible();
  await right.fill('queued for right only');
  await secondary.getByRole('button', { name: 'Send after this turn', exact: true }).click();
  const queue = secondary.getByRole('region', { name: 'Queued prompts' });
  await expect(queue).toContainText('queued for right only');
  await expect(primary.getByRole('region', { name: 'Queued prompts' })).toHaveCount(0);
  await queue.getByRole('button', { name: 'Steer', exact: true }).click();
  await expect(queue).toHaveCount(0);
  expect(requests.some(path => path.startsWith(`/api/threads/${b.id}/pending-steers/`) && path.endsWith('/steer'))).toBe(true);
  expect(requests.some(path => path.startsWith(`/api/threads/${a.id}/pending-steers/`))).toBe(false);
  await stop.click();
  await expect(stop).toHaveCount(0);
  expect(requests).toContain(`/api/threads/${b.id}/interrupt`);
  expect((await (await request.get(`${base}/api/threads/${a.id}`)).json()).thread.status).toBe('running');
  await primary.getByRole('button', { name: 'Stop Current Turn', exact: true }).click();
  await expect(primary.getByRole('button', { name: 'Stop Current Turn', exact: true })).toHaveCount(0);
  expect(requests).toContain(`/api/threads/${a.id}/interrupt`);
  await left.fill('untouched left draft');
  await right.fill('retry right draft');
  await page.route(`**/api/threads/${b.id}/prompt`, route => route.fulfill({ status: 400, json: { code: 'fixture_failure', message: 'Right conversation rejected this fixture' } }));
  await secondary.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect(secondary).toContainText('Right conversation rejected this fixture');
  await expect(primary).not.toContainText('Right conversation rejected this fixture');
  await expect(right).toHaveText('retry right draft');
  await expect(left).toHaveText('untouched left draft');
});

test('swapping primary while a send is in flight isolates the old error and busy state', async ({ page, request }) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  const { a, b } = await fixture(request);
  await page.goto(`/threads/${a.id}`);
  await selectSplit(page, b.id);
  const primary = page.getByTestId('primary-pane');
  const input = primary.getByRole('textbox', { name: 'Prompt', exact: true });
  const submit = primary.getByRole('button', { name: 'Send Prompt', exact: true });
  let releaseA!: () => void, releaseB!: () => void;
  const heldA = new Promise<void>(resolve => { releaseA = resolve; });
  const heldB = new Promise<void>(resolve => { releaseB = resolve; });
  const sent: string[] = [];
  await page.route('**/api/threads/*/prompt', async route => {
    const id = route.request().url().match(/threads\/([^/]+)\/prompt/)![1]!;
    sent.push(id);
    if (id === a.id) {
      await heldA;
      await route.fulfill({ status: 400, json: { code: 'late_a', message: 'Late A error must not reach B' } });
    } else { await heldB; await route.continue(); }
  });
  await input.fill('A before swap'); await submit.click();
  await expect.poll(() => sent).toEqual([a.id]);
  await page.getByTestId('make-primary').click();
  await expect(page).toHaveURL(new RegExp(`/threads/${b.id}$`));
  await input.fill('B after swap'); await submit.click();
  await expect.poll(() => sent).toEqual([a.id, b.id]);
  await expect(submit).toHaveAttribute('title', 'Sending...');
  const oldResponse = page.waitForResponse(response => response.url().endsWith(`/threads/${a.id}/prompt`));
  releaseA(); await oldResponse;
  await page.evaluate(() => new Promise<void>(done => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
  await expect(primary).not.toContainText('Late A error must not reach B');
  await expect(submit).toHaveAttribute('title', 'Sending...');
  await expect(input).toHaveText('B after swap');
  releaseB();
  await expect(input).toHaveText('');
  expect((await (await request.get(`${base}/api/threads/${a.id}`)).json()).turns).toHaveLength(1);
  await expect.poll(async () => (await (await request.get(`${base}/api/threads/${b.id}`)).json()).turns.length).toBe(2);
});
