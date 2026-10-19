import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type BrowserContext, type Page } from '@playwright/test';

const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 8787}`;
async function prepare(context: BrowserContext) {
  await context.addInitScript(() => {
    localStorage.setItem('remote-codex.locale', 'zh-CN');
    localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([window.location.origin, 'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} }));
  });
}
async function notifications(page: Page) {
  await page.goto('/workspaces');
  await page.getByRole('button', { name: '打开导航', exact: true }).click();
  await page.getByRole('button', { name: '设置', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: '设置', exact: true });
  await dialog.getByRole('tab', { name: /^通知/ }).click();
  await expect(dialog.getByRole('radio', { name: /仅订阅的轮次/ })).toBeEnabled();
  return dialog;
}

test('本地账号通知：跨浏览器订阅、结束失效、范围保存与工作区卡片', async ({ page, context, browser, request, baseURL }) => {
  await prepare(context);
  await request.put(`${api}/api/account/notifications/preferences`, { data: { scope: 'subscribed', devices: [], workspaces: [] } });
  const label = `通知工作区 ${randomUUID().slice(0, 6)}`;
  const absPath = path.join(process.env.E2E_WORKSPACE_ROOT!, `notify-${randomUUID()}`);
  await mkdir(absPath, { recursive: true });
  const workspace = await (await request.post(`${api}/api/workspaces`, { data: { absPath, label } })).json();
  const created = await (await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title: '通知订阅验证', model: 'ios-e2e-stream' } })).json();
  const thread = created.thread ?? created;
  const detail = async () => (await request.get(`${api}/api/threads/${thread.id}`)).json();
  const notices = async () => (await (await request.get(`${api}/api/account/notifications/events`)).json()).filter((e: { href: string }) => e.href.endsWith(thread.id));
  await page.goto(`/threads/${thread.id}`);
  await page.getByRole('textbox', { name: '提示词', exact: true }).fill('Inspect this repository for notification scope');
  await page.getByRole('button', { name: '发送提示词', exact: true }).click();
  const bell = page.getByRole('button', { name: '订阅这一轮的通知', exact: true });
  await expect(bell).toBeEnabled();
  await bell.click();
  await expect(page.getByRole('button', { name: '结束时通知你', exact: true })).toHaveAttribute('aria-pressed', 'true');
  const turn = (await detail()).thread.activeTurnId;
  expect(turn).toBeTruthy();
  await page.reload();
  await expect(page.getByRole('button', { name: '结束时通知你', exact: true })).toBeEnabled();
  const second = await browser.newContext({ baseURL });
  try {
    await prepare(second);
    const other = await second.newPage();
    await other.goto(`/threads/${thread.id}`);
    await expect(other.getByRole('button', { name: '结束时通知你', exact: true })).toBeEnabled();
    await other.getByRole('button', { name: '结束时通知你', exact: true }).click();
    await expect(other.getByRole('button', { name: '订阅这一轮的通知', exact: true })).toBeEnabled();
    await page.reload();
    await expect(bell).toBeEnabled();
    await bell.click();
    await expect(page.getByRole('button', { name: '结束时通知你', exact: true })).toBeEnabled();
  } finally { await second.close(); }
  await expect.poll(notices, { timeout: 40000 }).toHaveLength(1);
  expect(await (await request.get(`${api}/api/account/notifications/turns/${thread.id}/${turn}`)).json()).toEqual({ subscribed: false });
  expect((await request.put(`${api}/api/account/notifications/turns/${thread.id}/${turn}`, { data: { subscribed: true } })).status()).toBe(409);
  await request.post(`${api}/api/threads/${thread.id}/prompt`, { data: { prompt: 'hello' } });
  await expect.poll(async () => (await detail()).turns.length).toBe(2);
  await expect.poll(async () => (await detail()).thread.status).toBe('idle');
  expect(await notices()).toHaveLength(1);

  const dialog = await notifications(page);
  expect(await dialog.getByRole('tab', { name: /^通知/ }).getByLabel('账号设置，所有设备通用').count()).toBe(1);
  await dialog.getByRole('radio', { name: /所有轮次/ }).click();
  await expect(dialog.getByText('已保存', { exact: true })).toBeVisible();
  await page.reload();
  const saved = await notifications(page);
  await expect(saved.getByRole('radio', { name: /所有轮次/ })).toBeChecked();
  await request.post(`${api}/api/threads/${thread.id}/prompt`, { data: { prompt: 'hello again' } });
  await expect.poll(notices).toHaveLength(3);
  await saved.getByRole('radio', { name: /指定设备和工作区/ }).click();
  await expect(saved.getByText('已保存', { exact: true })).toBeVisible();
  await saved.getByRole('button', { name: '添加工作区', exact: true }).click();
  const picker = page.getByTestId('workbench-thread-picker');
  await picker.getByRole('button', { name: /此设备/ }).click();
  await picker.getByRole('button', { name: new RegExp(label) }).click();
  await expect(saved.getByRole('button', { name: `移除 ${label}`, exact: true })).toBeVisible();
  await expect(saved.getByText('已保存', { exact: true })).toBeVisible();
  const selected = await (await request.get(`${api}/api/account/notifications/preferences`)).json();
  expect(selected.workspaces).toEqual([{ deviceId: 'local', workspaceId: workspace.id, name: label, deviceName: '此设备' }]);
  await saved.getByRole('button', { name: `移除 ${label}`, exact: true }).click();
  await saved.getByRole('button', { name: '撤销', exact: true }).click();
  await expect(saved.getByRole('button', { name: `移除 ${label}`, exact: true })).toBeVisible();
});
