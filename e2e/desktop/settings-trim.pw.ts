import { test, expect, type Page } from '@playwright/test';
import { seed } from './fixtures';
import { join } from 'node:path';
const evidence = join(process.cwd(), '.scratch/settings-trim');
async function setup(page: Page, pack = 'classic', dark = false, width = 1440) {
  await page.setViewportSize({ width, height: 1000 });
  await page.addInitScript(() => { localStorage.setItem('pockymoe.onboarding.v1:' + JSON.stringify([location.origin, 'relay:devices-test']), JSON.stringify({welcomeDismissed:true,completed:[],resume:{}})); const url = new URL(location.href); url.searchParams.set('desktopMockUpdates', '1'); history.replaceState(null, '', url); });
  await seed(page, pack, dark);
  const { fallbackBackends } = await import('../../apps/supervisor-web/src/components/appShellNavigationModel');
  const backends = fallbackBackends.map(backend => ({ ...backend, capabilities: { ...backend.capabilities, management: { ...backend.capabilities.management, hostConfigFiles: backend.provider === 'codex' } }, managementSchema: { ...backend.managementSchema, hostConfigFiles: backend.provider === 'codex' ? [{ name: 'config.toml', label: 'config.toml', description: 'Harness config' }] : [], configArchives: backend.provider === 'codex' } }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user: { id: 'devices-test', username: 'Alex', email: 'alex@example.test' } } }));
  await page.route('**/relay/account/workbench', route => route.fulfill({ json: { threads: [], notifications: [] } }));
  await page.route('**/relay/access?*', route => route.fulfill({ json: { kind: 'owner' } }));
  await page.route('**/api/config/model-pricing', route => route.fulfill({ json: { models: {} } }));
  await page.route('**/api/agent-runtimes/*/agents*', route => route.fulfill({ json: [] }));
  await page.route('**/api/agent-runtimes', route => route.fulfill({ json: backends }));
  await page.route('**/api/config/providers/*/files/*', route => route.fulfill({ json: { path: '/fixture/config.toml', exists: true, content: '# fixture' } }));
  await page.route('**/api/config/providers/*/archives', route => route.fulfill({ json: [] }));
  await page.route('**/relay/devices/device-0/api/threads/t-0?*', route => route.fulfill({ json: {
    workspace: { id: 'w-0', label: 'Fixture', absPath: '/fixture' }, workspacePathStatus: 'present',
    thread: { id: 't-0', workspaceId: 'w-0', title: 'Settings trim', provider: 'acp', agentId: 'codex', providerSessionId: 'harness-session-fixture', source: 'supervisor', model: 'gpt-6.1', reasoningEffort: 'high', sandboxMode: 'workspace-write', status: 'idle', isLoaded: true, createdAt: '2026-11-01T12:00:00Z', updatedAt: '2026-11-01T12:00:00Z' }, turns: [], pendingRequests: [], pendingSteers: [],
  } }));
  await page.route('**/api/threads/*/capabilities*', route => route.fulfill({ json: { availability: 'available', effectiveCapabilities: { controls: {}, turns: {}, sessions: {}, branching: {}, management: {}, usage: {} }, negotiated: { harness: { notice: 'Harness fixture capabilities' } } } }));
  await page.route('**/api/threads/*/models*', route => route.fulfill({ json: [] }));
  await page.evaluate(() => { history.pushState({}, '', '/devices/device-0/threads/t-0'); dispatchEvent(new PopStateEvent('popstate')); });
  await expect(page.getByTestId('thread-scroll-container')).toBeVisible();
  await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
  await page.evaluate(() => document.fonts.ready);
  const later = page.getByRole('button', { name: '稍后', exact: true });
  if (await later.isVisible()) await later.click();
}
async function avatar(page: Page, width: number) {
  if (width < 640) {
    await page.getByRole('button', { name: '更多操作', exact: true }).click();
  }
  await page.getByRole('button', { name: /Alex.*Relay|Relay.*Alex|Alex.*账号|账号.*Alex/ }).click();
  return page.getByRole('menu');
}
for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const dark of [false, true]) for (const width of [1440, 390]) {
  test(`settings trim ${pack} ${dark ? 'dark' : 'light'} ${width}`, async ({ page }) => {
    await setup(page, pack, dark, width);
    const tag = `${pack}-${dark ? 'dark' : 'light'}-${width}`;
    await page.evaluate(() => window.dispatchEvent(new CustomEvent('desktopMock:update', { detail: { status: 'ready', version: '0.3.0' } })));
    const menu = await avatar(page, width);
    await expect(menu.locator('.client-version')).toHaveText('Pockymoe development · 可更新到 0.3.0');
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-avatar.png`) });
    if (pack === 'classic' && !dark && width === 1440) {
      await page.evaluate(() => { (window as any).avatarInstalls = 0; window.addEventListener('desktopMock:updateInstall', () => { (window as any).avatarInstalls++; }); });
      await menu.getByRole('menuitem', { name: '可更新到 0.3.0', exact: true }).click();
      expect(await page.evaluate(() => (window as any).avatarInstalls)).toBe(1);
      await expect(page.getByRole('button', { name: '更新到 0.3.0', exact: true })).toHaveAttribute('aria-busy', 'true');
    }
    await menu.getByRole('menuitem', { name: '设置', exact: true }).click();
    const settings = page.getByRole('dialog', { name: '设置', exact: true });
    await expect(settings.getByRole('tab', { name: '会话', exact: true })).toHaveCount(0);
    await expect(settings.getByRole('tab', { name: '高级', exact: true })).toHaveCount(0);
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-sections.png`) });
    await settings.getByRole('tab', { name: 'Harness', exact: true }).click();
    await expect(settings.getByText('config.toml', { exact: true })).toBeVisible();
    await settings.getByText('配置备份', { exact: true }).click();
    await expect(settings.getByRole('button', { name: '创建备份', exact: true })).toBeVisible();
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-harness.png`) });
    await settings.getByRole('button', { name: 'Claude Code', exact: true }).click();
    await expect(settings.getByText('此 Harness 没有可编辑的宿主文件。')).toBeVisible();
    await expect(settings.getByText('config.toml', { exact: true })).toHaveCount(0);
    await page.keyboard.press('Escape');
    if (width < 640) {
      await page.keyboard.press('Escape');
      await page.getByRole('button', { name: '更多操作', exact: true }).click();
      await page.getByRole('button', { name: '工作区与快捷方式', exact: true }).click();
    }
    const threadMenu = page.getByRole('button', { name: 'Settings trim 的操作', exact: true }).last();
    await threadMenu.evaluate(element => element.scrollIntoView({ block: 'center', behavior: 'instant' }));
    await threadMenu.hover();
    await threadMenu.click();
    const details = page.getByRole('button', { name: '会话详情', exact: true });
    await expect(details).toBeEnabled();
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-thread-menu.png`) });
    await details.click();
    const dialog = page.getByRole('dialog', { name: '会话详情', exact: true });
    await expect(dialog).toContainText('harness-session-fixture');
    await expect(dialog).toHaveCSS('opacity', '1');
    if (width < 640) await expect.poll(async () => { const box = await dialog.boundingBox(); return Math.abs(box!.y + box!.height - 1000); }).toBeLessThan(1);
    await expect(dialog.getByRole('button', { name: '复制 Harness 会话 ID', exact: true })).toBeVisible();
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-details.png`) });
    await page.keyboard.press('Escape');
    if (width < 640) await page.keyboard.press('Escape');
    await page.getByRole('textbox', { name: '提示词', exact: true }).click();
    await page.getByTestId('composer-model-label').click();
    const harnessEntry = page.getByRole('menuitem', { name: 'Harness 设置', exact: true });
    await expect(harnessEntry).toBeVisible();
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-model-menu.png`) });
    await harnessEntry.click();
    const controls = page.getByRole('dialog', { name: 'Harness 设置', exact: true });
    await expect(controls.getByRole('combobox', { name: '工作区权限', exact: true })).toBeVisible();
    await expect(controls).toContainText('Harness fixture capabilities');
    await expect(controls).toHaveCSS('opacity', '1');
    if (width < 640) await expect.poll(async () => { const box = await controls.boundingBox(); return Math.abs(box!.y + box!.height - 1000); }).toBeLessThan(1);
    await page.screenshot({ animations: 'disabled', path: join(evidence, `${tag}-thread-settings.png`) });
  });
}

for (const width of [390, 1440]) test(`secondary overlays are exclusive ${width}`, async ({ page }) => {
  await setup(page, 'classic', false, width);
  await page.evaluate(() => window.dispatchEvent(new CustomEvent('desktopMock:update', { detail: { status: 'ready', version: '0.3.0' } })));
  if (width < 640) {
    await page.getByRole('button', { name: '更多操作', exact: true }).click();
    await expect(page.locator('.matter-phone-sheet[data-open]')).toHaveCount(1);
  }
  await page.getByRole('button', { name: 'Alex 的 Relay 账号菜单', exact: true }).click();
  await expect(page.getByRole('menu')).toBeVisible();
  await expect(page.locator('.matter-phone-sheet[data-open]')).toHaveCount(0);
  await page.screenshot({ animations: 'disabled', path: join(evidence, `exclusive-${width}-avatar.png`) });
  await page.screenshot({ animations: 'disabled', path: join(evidence, `classic-light-${width}-avatar.png`) });
  await page.keyboard.press('Escape');
  await expect(page.getByRole('menu')).toHaveCount(0);
  if (width < 640) {
    await expect(page.getByRole('button', { name: '更多操作', exact: true })).toBeFocused();
    await avatar(page, width);
    await page.getByRole('button', { name: '更多操作', exact: true }).click();
    await expect(page.getByRole('menu')).toHaveCount(0);
    await expect(page.locator('.matter-phone-sheet[data-open]')).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(page.locator('.matter-phone-sheet[data-open]')).toHaveCount(0);
  } else {
    await avatar(page, width);
    await page.getByRole('button', { name: '线程工具', exact: true }).click();
    await expect(page.getByRole('menu')).not.toContainText('alex@example.test');
    await expect(page.locator('[data-account-rail][aria-expanded="true"]')).toHaveCount(0);
    await expect(page.getByRole('menu', { name: '线程工具', exact: true })).toBeVisible();
    await avatar(page, width);
    await expect(page.getByRole('menu', { name: '线程工具', exact: true })).toHaveCount(0);
    await expect(page.getByRole('menu')).toContainText('alex@example.test');
    await page.keyboard.press('Escape');
    await expect(page.getByRole('menu')).toHaveCount(0);
  }
});
