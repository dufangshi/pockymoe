import { test, expect, type Page, type APIRequestContext } from '@playwright/test';
import { randomUUID } from 'node:crypto';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
const api = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 19182}`;
const evidence = path.resolve('.scratch/mobile-host/screenshots');

async function host(page: Page, relay = false, relayOrigin?: string) {
  await page.addInitScript(({ relay, relayOrigin }) => {
    localStorage.setItem('remote-codex.locale', 'en');
    if (relay) window.__POCKYMOE_BOOTSTRAP__ = { mode: 'relay' };
    const listeners = new Set<(command: any) => void>();
    const state = { snapshots: [] as any[], appearances: [] as any[], results: {} as Record<string, any>, haptics: [] as string[], auth: [] as string[] };
    Object.assign(window, { mobileEvidence: state, mobileCommand: (command: any) => listeners.forEach(listener => listener(command)) });
    window.pockymoeHost = {
      capabilities: async () => ({ surface: 'mobile', platform: 'ios', liveActivity: true, nativePush: true, haptics: true }),
      activity: { sync: snapshot => state.snapshots.push(snapshot) }, appearance: { sync: appearance => state.appearances.push(appearance) },
      onCommand: listener => { listeners.add(listener); return () => { listeners.delete(listener); }; },
      commandResult: (id, result) => { state.results[id] = result; },
      haptics: { play: kind => state.haptics.push(kind) },
      push: { register: async () => ({ token: 'ab'.repeat(32), sandbox: true, kind: 'ios' }) },
      auth: { openExternal: async url => { state.auth.push(url); if ((window as any).mobileCancelAuth) return null; return { callbackUrl: `remotecodex://auth/callback?code=${'c'.repeat(43)}` }; } },
      window: { setTitle: () => {}, openThread: path => { history.pushState(null, '', path); dispatchEvent(new PopStateEvent('popstate')); }, openPreferences: () => {} },
      badge: { set: () => {} }, shell: { openExternal: () => {} }, files: { saveAs: async () => ({ saved: false }) },
      pet: { setVisible: () => {}, isVisible: async () => false }, settings: { get: async () => ({ webOrigin: location.origin, relayOrigin: relayOrigin ?? location.origin }), set: async () => ({}) },
    } as any;
  }, { relay, relayOrigin });
}
async function fixture(request: APIRequestContext) {
  const absPath = path.resolve(process.env.E2E_WORKSPACE_ROOT!, randomUUID()); await mkdir(absPath, { recursive: true });
  const response = await request.post(`${api}/api/workspaces`, { data: { absPath, label: 'Mobile host' } }); expect(response.ok()).toBeTruthy();
  const workspace = await response.json();
  const started = await request.post(`${api}/api/threads/start`, { data: { workspaceId: workspace.id, title: 'Mobile bridge verification', provider: 'acp', agentId: 'codex', model: 'ios-e2e-stream', approvalMode: 'yolo' } });
  expect(started.ok()).toBeTruthy(); const value = await started.json(); return { thread: value.thread ?? value, workspace };
}
async function command(page: Page, value: object) { await page.evaluate(value => (window as any).mobileCommand(value), value); }

test('mobile host retains phone UI, sends haptics, closes sheets before history and syncs all themes', async ({ page, request }) => {
  const { thread } = await fixture(request); await host(page);
  await page.addInitScript(() => localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin,'local:owner'])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })));
  await page.goto(`/threads/${thread.id}`);
  const composer = page.getByTestId('chat-composer'); await expect(composer).toBeVisible();
  await expect(page.locator('.desktop-window-frame, .desktop-shell, .desktop-rail')).toHaveCount(0);
  const editor = composer.getByRole('textbox', { name: 'Prompt', exact: true });
  await editor.fill('Mobile host hello');
  await composer.getByRole('button', { name: 'Send Prompt', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.haptics.filter((k: string) => k === 'sent').length)).toBe(1);
  await expect(page.getByText('Mobile host hello', { exact: true }).first()).toBeVisible();
  // Real phone bottom sheet uses the shared Escape/focus behavior.
  const more = page.getByRole('button', { name: 'More actions', exact: true }); await more.click();
  await expect(page.getByRole('dialog').or(page.getByRole('menu')).first()).toBeVisible();
  const before = page.url(); await command(page, { id: 'sheet', type: 'back' });
  await expect(page.getByRole('dialog').or(page.getByRole('menu'))).toHaveCount(0); expect(page.url()).toBe(before);
  await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.results.sheet)).toEqual({ ok: true, data: { handled: true } });
  await command(page, { id: 'root', type: 'back' });
  await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.results.root)).toEqual({ ok: true, data: { handled: false } });
  await mkdir(evidence, { recursive: true });
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.evaluate(({ pack, mode }) => { localStorage.setItem('pockymoe-theme-preset',pack); localStorage.setItem('remote-codex-theme-mode',mode); }, { pack, mode });
    await page.reload(); await expect(composer).toBeVisible();
    await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.appearances.at(-1)?.pack)).toBe(pack);
    const appearance = await page.evaluate(() => (window as any).mobileEvidence.appearances.at(-1)); expect(appearance.effective).toBe(mode); expect(appearance.background).toBeTruthy();
    await page.screenshot({ path: path.join(evidence, `${test.info().project.name}-${pack}-${mode}.png`) });
    await page.setViewportSize({ width: 1440, height: 1000 }); await expect(page.locator('.desktop-window-frame, .desktop-shell, .desktop-rail')).toHaveCount(0);
    await page.screenshot({ path: path.join(evidence, `wide-${pack}-${mode}.png`) });
    await page.setViewportSize({ width: 393, height: 851 });
  }
});

test('mobile account activity, native registration and token rotation run without desktop chrome', async ({ page, request }) => {
  const { thread, workspace } = await fixture(request); await host(page, true);
  const user = { id: 'mobile-test', username: 'Mobile', role: 'user', enabled: true };
  await page.route('**/src/components/AccountSessionGate.tsx*', route => route.fulfill({ contentType: 'application/javascript', body: 'export function AccountSessionGate({children}) { return children; }' }));
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: true, user } }));
  await page.route('**/relay/portal', route => route.fulfill({ json: { user, devices: [{ id: 'device', name: 'Fixture Mac', ownerUserId: user.id, connected: true }], sharedWithMe: [], sharedByMe: [] } }));
  await page.route('**/relay/devices/device/presence*', route => route.fulfill({ json: { connected: true } }));
  await page.route('**/relay/account/workbench**', route => route.fulfill({ json: { threads: [], notifications: [] } }));
  await page.route('**/relay/account/notifications/preferences', route => route.fulfill({ json: { scope: 'all', devices: [], workspaces: [] } }));
  await page.route('**/relay/devices/device/api/**', async route => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/transport/key')) return route.fulfill({ status: 404, json: { code: 'not_found' } });
    if (url.pathname.endsWith('/api/workspaces')) return route.fulfill({ json: [workspace] });
    if (url.pathname.endsWith('/api/threads')) return route.fulfill({ json: [{ ...thread, status: 'running', activeTurnId: 'turn', lastTurnStartedAt: new Date().toISOString() }] });
    if (url.pathname.includes(`/threads/${thread.id}`)) return route.fulfill({ json: { thread, workspace, turns: [], pendingRequests: [], pendingSteers: [] } });
    return route.fulfill({ json: {} });
  });
  await page.routeWebSocket(/\/relay\/devices\/.*\/ws/, socket => socket.onMessage(() => socket.send(JSON.stringify({ type: 'supervisor.pong' }))));
  await page.route('**/relay/account/notifications', route => route.fulfill({ json: { publicKey: '', scope: 'all', nativePushAvailable: true, subscriptionIds: ['mobile-registration'] } }));
  await page.addInitScript(() => localStorage.setItem(`pockymoe.mobile-push:${location.origin}:mobile-test`, 'mobile-registration'));
  const registrations: any[] = [], rotations: any[] = [];
  await page.route('**/relay/account/notifications/native', route => { registrations.push(route.request().postDataJSON()); return route.fulfill({ json: { id: 'mobile-registration' } }); });
  await page.route('**/relay/account/notifications/native/live-activity', route => { rotations.push(route.request().postDataJSON()); return route.fulfill({ json: { ok: true } }); });
  await page.goto('/relay-devices');
  await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.snapshots.at(-1)?.items.length)).toBe(1);
  const snapshot = await page.evaluate(() => (window as any).mobileEvidence.snapshots.at(-1));
  expect(snapshot.items[0]).toMatchObject({ threadId: thread.id, state: 'running', deviceName: 'Fixture Mac' });
  await expect(page.locator('.desktop-window-frame, .desktop-shell, .desktop-rail')).toHaveCount(0);
  await expect.poll(() => registrations.length).toBe(1);
  await command(page, { id: 'token', type: 'liveActivityToken', token: 'cd'.repeat(32) });
  await expect.poll(() => rotations.length).toBe(1); expect(rotations[0]).toEqual({ id: 'mobile-registration', token: 'cd'.repeat(32) });
  await expect.poll(() => page.evaluate(() => (window as any).mobileEvidence.results.token)).toEqual({ ok: true });
});

test('Google and GitHub open system auth and redeem only the matching one-use callback', async ({ page }) => {
  const shellRelay = 'https://shell-relay.example.test';
  await host(page, true, shellRelay);
  const authOrigins: string[] = []; page.on('request', request => { if (request.url().includes('/relay/auth/')) authOrigins.push(new URL(request.url()).origin); });
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false, user: null, registrationSettings: { googleAuthEnabled: true, githubAuthEnabled: true }, registrationEnabled: true } }));
  const prepared: any[] = [], exchanged: any[] = [];
  await page.route('**/relay/auth/mobile/prepare', route => { const body = route.request().postDataJSON(); prepared.push(body); return route.fulfill({ json: { path: `/relay/auth/oauth/${body.provider}/start?mobileId=fixture`, callbackUrl: 'remotecodex://auth/callback' } }); });
  await page.route('**/relay/auth/mobile/exchange', route => { exchanged.push(route.request().postDataJSON()); return route.fulfill({ json: { path: '/relay-portal?oauthPending=1' } }); });
  for (const provider of ['Google', 'GitHub']) {
    await page.goto('/relay-portal');
    await page.getByRole('button', { name: `Continue with ${provider}` }).click();
    await expect.poll(() => exchanged.length).toBe(provider === 'Google' ? 1 : 2);
    await expect(page.getByText(/registration.*received|admin.*approve/i).first()).toBeVisible();
    expect(prepared.at(-1).provider).toBe(provider.toLowerCase()); expect(prepared.at(-1).proofHash).toHaveLength(43);
    expect(exchanged.at(-1)).toMatchObject({ code: 'c'.repeat(43) }); expect(exchanged.at(-1)).not.toHaveProperty('id'); expect(exchanged.at(-1).proof).toHaveLength(64);
    expect(authOrigins.length).toBeGreaterThan(0); expect(authOrigins.every(origin => origin === shellRelay)).toBe(true);
  }
});

test('browser and desktop retain their existing signed-out layout', async ({ page }) => {
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false, registrationSettings: { googleAuthEnabled: true } } }));
  await page.goto('/relay-portal');
  await expect(page.getByRole('link', { name: 'Continue with Google' })).toBeVisible();
  await expect(page.locator('.desktop-window-frame')).toHaveCount(0);
  await page.goto('/relay-portal?desktopMockHost=1');
  await expect(page.locator('.desktop-window-frame')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Continue with Google' })).toBeVisible();
});


test('cancelled system sign-in shows its failure across phone and desktop theme appearances', async ({ page }) => {
  await host(page, true);
  await page.addInitScript(() => { (window as any).mobileCancelAuth = true; });
  await page.route('**/relay/auth/session', route => route.fulfill({ json: { authenticated: false, user: null, registrationSettings: { googleAuthEnabled: true }, registrationEnabled: true } }));
  await page.route('**/relay/auth/mobile/prepare', route => route.fulfill({ json: { path: '/relay/auth/oauth/google/start?mobileId=fixture', callbackUrl: 'remotecodex://auth/callback' } }));
  let exchanges = 0;
  await page.route('**/relay/auth/mobile/exchange', route => { exchanges++; return route.fulfill({ status: 401, json: {} }); });
  await mkdir(path.resolve('.scratch/security-review/screenshots'), { recursive: true });
  await page.goto('/relay-portal');
  for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
    await page.evaluate(({ pack, mode }) => { localStorage.setItem('pockymoe-theme-preset', pack); localStorage.setItem('remote-codex-theme-mode', mode); }, { pack, mode });
    await page.reload();
    await page.getByRole('button', { name: 'Continue with Google' }).click();
    const failure = page.locator('.auth-notice[data-tone="warning"]');
    await expect(failure).toContainText(/cancelled/i);
    for (const width of [393, 1440]) {
      await page.setViewportSize({ width, height: 900 }); await expect(failure).toBeVisible();
      await page.screenshot({ path: path.resolve(`.scratch/security-review/screenshots/auth-${pack}-${mode}-${width}.png`) });
    }
  }
  expect(exchanges).toBe(0);
});
