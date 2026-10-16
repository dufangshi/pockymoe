import { test, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { randomBytes } from 'node:crypto';

test.use({ actionTimeout: 15_000 });
const screenshots = process.env.WORKBENCH_SCREENSHOT_DIR;
async function snapshot(page: Page, name: string) {
  if (!screenshots) return;
  await mkdir(screenshots, { recursive: true });
  await page.screenshot({
    path: join(screenshots, name),
    scale: 'css',
    animations: 'disabled',
  });
}

test('cross-device split keeps both chats writable and tools follow the last focused workspace safely', async ({
  browser,
}, testInfo) => {
  test.setTimeout(150_000);
  const mobile = testInfo.project.name === 'mobile-chromium';
  const root = await mkdtemp(resolve('.temp/workbench/targets-'));
  const processes: ChildProcess[] = [];
  const freePort = () =>
    new Promise<number>((done) => {
      const server = createServer();
      server.listen(0, '127.0.0.1', () => {
        const port = (server.address() as { port: number }).port;
        server.close(() => done(port));
      });
    });
  const relayPort = await freePort(),
    base = `http://127.0.0.1:${relayPort}`;
  const password = randomBytes(24).toString('hex');
  const environment = Object.fromEntries(
    Object.entries(process.env).filter(
      ([key]) => !/^(POCKYMOE|REMOTE_CODEX)_/.test(key),
    ),
  );
  const start = (command: string, extra: Record<string, string>) => {
    const proc = spawn(resolve('target/debug/pockymoe'), [command], {
      env: {
        ...environment,
        HOST: '127.0.0.1',
        POCKYMOE_E2E_FAKE_RUNTIME: '1',
        POCKYMOE_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_RELAY_SESSION_SECRET: randomBytes(32).toString('hex'),
        POCKYMOE_ADMIN_USERNAME: 'admin',
        POCKYMOE_ADMIN_PASSWORD: password,
        ...extra,
      },
      stdio: 'ignore',
    });
    processes.push(proc);
  };
  const api = async (path: string, token?: string, body?: unknown) => {
    const response = await fetch(`${base}${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: {
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(15_000),
    });
    expect(
      response.ok,
      `${path}: ${response.status} ${response.ok ? '' : await response.text()}`,
    ).toBeTruthy();
    return response.json();
  };
  const context = await browser.newContext({
    viewport: mobile
      ? { width: 390, height: 844 }
      : { width: 1440, height: 1000 },
    ...(mobile ? { isMobile: true, hasTouch: true } : {}),
  });
  // The classic phone bars and composer split switches are under test here.
  await context.addInitScript(() => localStorage.setItem('pockymoe.phoneDock', 'off'));
  try {
    start('relay', {
      PORT: String(relayPort),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
      POCKYMOE_PUBLIC_BASE_URL: base,
    });
    await expect
      .poll(() =>
        fetch(`${base}/healthz`)
          .then((r) => r.status)
          .catch(() => 0),
      )
      .toBe(200);
    await api('/relay/auth/register', undefined, {
      username: 'owner',
      email: 'owner@example.test',
      password,
    });
    const owner = (
      await api('/relay/auth/login', undefined, { username: 'owner', password })
    ).token;
    const first = await api('/relay/devices', owner, { name: '开发设备' }),
      second = await api('/relay/devices', owner, { name: '验证设备' });
    for (const [index, device] of [first, second].entries()) {
      const port = await freePort();
      start('relay-supervisor', {
        PORT: String(port),
        POCKYMOE_RELAY_SUPERVISOR_PORT: String(port),
        POCKYMOE_RELAY_SERVER_URL: base,
        POCKYMOE_RELAY_AGENT_TOKEN: device.token,
        POCKYMOE_DATABASE_PATH: join(root, `device-${index}.sqlite`),
        POCKYMOE_WORKSPACE_ROOT: join(root, `device-${index}`),
      });
    }
    await expect
      .poll(async () => (await api('/healthz')).connectedSupervisors)
      .toBe(2);
    const deviceA = first.device.id,
      deviceB = second.device.id;
    const prefix = (device: string) => `/relay/devices/${device}/api`;
    const workspace = async (device: string, label: string, name: string) => {
      const absPath = join(root, name);
      await mkdir(absPath, { recursive: true });
      return api(`${prefix(device)}/workspaces`, owner, { absPath, label });
    };
    const wsA = await workspace(deviceA, '产品工作区', 'product'),
      wsOther = await workspace(deviceA, '文档工作区', 'docs'),
      wsB = await workspace(deviceB, '验收工作区', 'review');
    await writeFile(
      join(wsA.absPath, 'from-a.md'),
      '# 产品工作区\n\n只属于开发设备。\n',
    );
    await writeFile(
      join(wsB.absPath, 'from-b.md'),
      '# 验收工作区\n\n只属于验证设备。\n',
    );
    const thread = async (
      device: string,
      ws: { id: string },
      title: string,
    ) => {
      const value = await api(`${prefix(device)}/threads/start`, owner, {
        workspaceId: ws.id,
        provider: 'codex',
        model: 'default',
        title,
        approvalMode: 'yolo',
      });
      return value.thread ?? value;
    };
    const a = await thread(deviceA, wsA, '实现 · 产品工作台'),
      same = await thread(deviceA, wsA, '同工作区 · 代码审查'),
      other = await thread(deviceA, wsOther, '文档 · 发布说明'),
      b = await thread(deviceB, wsB, '验证 · 独立验收');
    for (const [device, value] of [
      [deviceA, a],
      [deviceB, b],
    ] as const) {
      await api(`${prefix(device)}/threads/${value.id}/prompt`, owner, {
        prompt: '检查工作台布局和独立发送目标。',
      });
      await expect
        .poll(
          async () =>
            (await api(`${prefix(device)}/threads/${value.id}`, owner)).thread
              .status,
        )
        .toBe('idle');
    }
    await context.addCookies([
      { name: 'remote_codex_relay_session', value: owner, url: base },
    ]);
    await context.addInitScript(() => {
      localStorage.setItem('remote-codex.locale', 'zh-CN');
      localStorage.setItem('remote-codex-theme-mode', 'dark');
    });
    const page = await context.newPage();
    // The first-run onboarding card would cover the workbench controls.
    await page.addLocatorHandler(page.locator('.pm-tour-welcome'), async () => {
      await page.getByRole('button', { name: '稍后', exact: true }).click();
    });
    await page.goto(`${base}/devices/${deviceA}/threads/${a.id}`);
    const primary = page.getByTestId('primary-pane'),
      secondary = page.getByTestId('reference-pane');
    await expect(
      primary.getByRole('textbox', { name: '提示词', exact: true }),
    ).toBeVisible();
    await expect(page.locator('.workbench-context')).toHaveCount(0);
    await expect(page.getByRole('combobox', { name: /对照|分屏/ })).toHaveCount(
      0,
    );
    const choose = async () =>
      page.getByTestId('workbench-split-trigger').click();
    await choose();
    const picker = page.getByTestId('workbench-thread-picker');
    await expect(picker).toHaveCSS('background-color', 'rgb(15, 19, 23)');
    await expect(picker.locator(`[data-thread-id="${same.id}"]`)).toBeVisible();
    await expect(
      picker.locator(`[data-workspace-id="${wsOther.id}"]`),
    ).toBeVisible();
    await expect(picker.locator(`[data-device-id="${deviceB}"]`)).toBeVisible();
    await snapshot(page, mobile ? 'mobile-picker.png' : 'desktop-picker.png');
    await picker.locator(`[data-workspace-id="${wsOther.id}"]`).click();
    await expect(
      picker.locator(`[data-thread-id="${other.id}"]`),
    ).toBeVisible();
    await picker
      .getByRole('button', { name: '返回上一级', exact: true })
      .click();
    await picker.locator(`[data-device-id="${deviceB}"]`).click();
    await picker.locator(`[data-workspace-id="${wsB.id}"]`).click();
    await picker.locator(`[data-thread-id="${b.id}"]`).click();
    const left = primary.getByRole('textbox', { name: '提示词', exact: true }),
      right = secondary.getByRole('textbox', { name: '提示词', exact: true });
    // Phone splits switch the conversations beside the composer's timeline capsule.
    const showLeft = async () => {
      if (mobile)
        await page.locator('.thread-pane-switch[data-side="primary"]:visible').click();
    };
    const showRight = async () => {
      if (mobile)
        await page.locator('.thread-pane-switch[data-side="reference"]:visible').click();
    };
    await showRight();
    await expect(right).toBeVisible();
    await expect(right).toHaveAttribute(
      'contenteditable',
      /^(true|plaintext-only)$/,
    );
    await right.fill('验证设备的独立草稿');
    await snapshot(
      page,
      mobile ? 'mobile-second-chat.png' : 'desktop-dual-chat.png',
    );
    const openFiles = async () => {
      if (mobile) {
        await page
          .getByRole('button', { name: '切换快捷方式侧栏', exact: true })
          .click();
        await page
          .getByRole('button', { name: '打开文件浏览器', exact: true })
          .click();
      } else
        await page
          .getByRole('navigation', { name: '工作区工具' })
          .getByRole('button', { name: '切换文件浏览器', exact: true })
          .click();
    };
    await openFiles();
    const target = page.getByTestId('workbench-tool-target');
    await expect(target).toHaveAttribute('data-device', deviceB);
    await expect(target).toHaveAttribute('data-workspace', wsB.id);
    const file = page.getByRole('treeitem', { name: 'from-b.md', exact: true });
    await expect(file).toBeVisible();
    await expect(
      page.getByRole('treeitem', { name: 'from-a.md', exact: true }),
    ).toHaveCount(0);
    await file.focus();
    await file.press('Enter');
    await page.getByRole('button', { name: '编辑文件', exact: true }).click();
    const editor = page.getByRole('textbox', {
      name: mobile ? '工作区文件编辑器' : '工作区编辑器：from-b.md',
      exact: true,
    });
    await editor.focus();
    await editor.press('ControlOrMeta+End');
    await page.keyboard.insertText('\n焦点绑定后的独立草稿。');
    await expect(page.getByTestId('workspace-document-status')).toContainText(
      '未保存草稿',
    );
    await snapshot(
      page,
      mobile ? 'mobile-focused-files.png' : 'desktop-focused-files.png',
    );
    await page.getByTestId('workbench-close-files').click();
    page.once('dialog', (dialog) => dialog.dismiss());
    if (mobile) await showLeft();
    else await left.click();
    // Cancelling a source switch keeps the file adapter and draft on device B.
    await openFiles();
    await expect(target).toHaveAttribute('data-device', deviceB);
    if (mobile) await expect(editor).toHaveValue(/独立草稿/);
    else
      await expect(page.getByTestId('workspace-monaco-editor')).toContainText(
        '独立草稿',
      );
    await page.getByRole('button', { name: '保存文件', exact: true }).click();
    await expect
      .poll(() => readFile(join(wsB.absPath, 'from-b.md'), 'utf8'))
      .toContain('焦点绑定后的独立草稿');
    expect(
      await readFile(join(wsA.absPath, 'from-a.md'), 'utf8'),
    ).not.toContain('独立草稿');
    await page.getByTestId('workbench-close-files').click();
    await showLeft();
    await left.click();
    await openFiles();
    await expect(target).toHaveAttribute('data-device', deviceA);
    if (mobile) {
      await expect(
        page.getByRole('tab', { name: 'from-a.md', exact: true }),
      ).toBeVisible();
      await expect(
        target.getByRole('heading', { name: '产品工作区', exact: true }),
      ).toBeVisible();
    } else
      await expect(
        page.getByRole('treeitem', { name: 'from-a.md', exact: true }),
      ).toBeVisible();
    await page.getByTestId('workbench-close-files').click();
    const terminalToggle = page.getByRole('button', {
      name: '终端',
      exact: true,
    });
    await terminalToggle.click();
    await expect(page.getByTestId('workbench-terminal-target')).toHaveAttribute(
      'data-device',
      deviceA,
    );
    await page.getByTestId('workbench-close-tools').click();
    await showRight();
    await right.click();
    await terminalToggle.click();
    await expect(page.getByTestId('workbench-terminal-target')).toHaveAttribute(
      'data-device',
      deviceB,
    );
    await snapshot(
      page,
      mobile ? 'mobile-focused-terminal.png' : 'desktop-focused-terminal.png',
    );
    await page.getByTestId('workbench-close-tools').click();
    await secondary
      .getByRole('button', { name: '发送提示词', exact: true })
      .click();
    await showLeft();
    await left.fill('开发设备的独立消息');
    await primary
      .getByRole('button', { name: '发送提示词', exact: true })
      .click();
    for (const [device, value] of [
      [deviceA, a],
      [deviceB, b],
    ] as const)
      await expect
        .poll(
          async () =>
            (await api(`${prefix(device)}/threads/${value.id}`, owner)).turns
              .length,
        )
        .toBe(2);
    await showRight();
    await right.click();
    await expect
      .poll(
        async () =>
          (await api(`${prefix(deviceB)}/threads/${b.id}`, owner)).thread
            .status,
      )
      .toBe('idle');
    await page.getByRole('button', { name: '打开设置', exact: true }).click();
    await page.getByTestId('settings-dialog').getByRole('tab', {name:'会话',exact:true}).click();
    const sessionSettings = page.getByTestId('workbench-session-settings');
    await expect(sessionSettings).toHaveAttribute('data-device', deviceB);
    await expect(sessionSettings).toHaveAttribute('data-thread', b.id);
    const permissions = sessionSettings.getByRole('combobox', {
      name: '工作区权限',
      exact: true,
    });
    await expect(permissions).toHaveValue('danger-full-access');
    await permissions.selectOption('workspace-write');
    await expect
      .poll(
        async () =>
          (await api(`${prefix(deviceB)}/threads/${b.id}`, owner)).thread
            .sandboxMode,
      )
      .toBe('workspace-write');
    expect(
      (await api(`${prefix(deviceA)}/threads/${a.id}`, owner)).thread
        .sandboxMode,
    ).not.toBe('workspace-write');
    await snapshot(
      page,
      mobile ? 'mobile-session-settings.png' : 'desktop-session-settings.png',
    );
    await expect(page).toHaveURL(
      new RegExp(`/devices/${deviceA}/threads/${a.id}$`),
    );
    await page.reload();
    await showRight();
    await expect(right).toBeVisible();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth,
      ),
    ).toBe(true);
  } finally {
    await context.close();
    await Promise.all(
      processes.map(
        (proc) =>
          new Promise<void>((done) => {
            if (proc.exitCode !== null) {
              done();
              return;
            }
            proc.once('exit', () => done());
            proc.kill('SIGTERM');
            setTimeout(() => {
              if (proc.exitCode === null) proc.kill('SIGKILL');
              done();
            }, 3000).unref();
          }),
      ),
    );
  }
});
