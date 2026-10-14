import { createRequire } from 'node:module';
import { read, origin, sessionCookie, treerSource } from './common.mjs';
const require = createRequire(`${treerSource}/web/package.json`);
const { test, expect } = require('@playwright/test');

test('one-command setup profile opens its authenticated bound thread UI', async ({ page, context }) => {
  const state = await read('setup-browser.json');
  const base = `${origin}/api/workspaces/${state.workspaceId}/agents/${state.agentId}/interface/ui/`;
  const anonymous = await context.request.get(base, { headers: { Origin: origin } });
  expect(anonymous.status()).toBe(401);
  const cookie = (await sessionCookie()).split('; ').find(pair => pair.startsWith('treer_session='));
  expect(Boolean(cookie), 'Real authenticated Treer lab session').toBe(true);
  await context.addCookies([{ name: 'treer_session', value: cookie.slice('treer_session='.length),
    url: origin, httpOnly: true, sameSite: 'Lax' }]);
  const redirect = await context.request.get(base, { maxRedirects: 0, headers: { Origin: origin } });
  expect(redirect.status()).toBe(307);
  expect(redirect.headers().location).toBe(`threads/${state.threadId}`);
  const response = await page.goto(base);
  expect(response.status()).toBe(200);
  await expect(page).toHaveURL(`${base}threads/${state.threadId}`);
  await expect(page.getByText(state.promptText, { exact: false }).first()).toBeVisible();
  const detail = await context.request.get(`${base}api/threads/${state.threadId}`, { headers: { Origin: origin } });
  expect(detail.status()).toBe(200);
  expect((await detail.json()).thread.id).toBe(state.threadId);
});
