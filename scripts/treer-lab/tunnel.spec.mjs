import { createRequire } from 'node:module';
import { read, origin, sessionCookie, treerSource } from './common.mjs';
const require = createRequire(`${treerSource}/web/package.json`);
const { test, expect } = require('@playwright/test');

test('Treer authenticated UI renders its bound thread and rejects another thread API', async ({ page, context }) => {
  const state = await read('browser.json');
  const tunnel = agent => `${origin}/api/workspaces/${state.workspaceId}/agents/${agent}/interface/ui/`;
  const anonymous = await context.request.get(tunnel(state.firstAgentId), { headers: { Origin: origin } });
  expect(anonymous.status()).toBe(401);
  const cookie = (await sessionCookie()).split('; ').find(pair => pair.startsWith('treer_session='));
  expect(Boolean(cookie), 'Real Treer user session is required').toBe(true);
  await context.addCookies([{ name: 'treer_session', value: cookie.slice('treer_session='.length),
    url: origin, httpOnly: true, sameSite: 'Lax' }]);
  const firstBase = tunnel(state.firstAgentId);
  const secondBase = tunnel(state.secondAgentId);
  const response = await page.goto(firstBase);
  expect(response.status()).toBe(200);
  await expect(page.getByText('Treer lab first turn', { exact: false }).first()).toBeVisible();
  // Learn IDs through each legitimately authorized Agent entry, then attempt cross-entry access.
  const threads = async base => {
    const result = await context.request.get(`${base}api/threads`, { headers: { Origin: origin } });
    expect(result.status()).toBe(200);
    const body = await result.json();
    return Array.isArray(body) ? body : body.threads;
  };
  const first = await threads(firstBase);
  const second = await threads(secondBase);
  expect(first.length).toBeGreaterThan(0);
  expect(second.length).toBeGreaterThan(0);
  const redirect = await context.request.get(firstBase, { maxRedirects: 0, headers: { Origin: origin } });
  expect(redirect.status()).toBe(307);
  expect(redirect.headers().location).toBe(`threads/${first[0].id}`);
  await expect(page).toHaveURL(`${firstBase}threads/${first[0].id}`);
  const other = second.find(thread => !first.some(own => own.id === thread.id));
  expect(Boolean(other), 'Each Agent must expose a different bound thread').toBe(true);
  const denied = await context.request.get(`${firstBase}api/threads/${other.id}`, { headers: { Origin: origin } });
  expect([403, 404]).toContain(denied.status());
  const own = await context.request.get(`${firstBase}api/threads/${first[0].id}`, { headers: { Origin: origin } });
  expect(own.status()).toBe(200);
});
