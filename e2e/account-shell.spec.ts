import { test, expect, _electron as electron, type Page } from '@playwright/test';
import { fixture, confirmRecoveryCode, readRecoveryCode, unlock, UNLOCK_DIALOG } from './helpers/account-key-fixture';
import { createRequire } from 'node:module';
import { resolve, join } from 'node:path';
import { mkdirSync, writeFileSync } from 'node:fs';
const evidence = '.scratch/account-shell';
const appRoot = process.env.POCKYMOE_ELECTRON_APP_ROOT;
async function capture(page: Page, name: string) {
  await page.evaluate(() => document.fonts.ready);
  if (name.includes('account')) await expect(page.locator('[data-page="account"] code').first()).not.toBeEmpty();
  if (name.endsWith('menu')) await expect(page.getByRole('menu')).toBeVisible();
  if (name.includes('phone') && (name.endsWith('devices') || name.endsWith('shared'))) await expect(page.locator('.product-topbar').getByRole('button', { name: /Relay account menu/ })).toBeVisible();
  await page.screenshot({ path: `${evidence}/${name}.png`, fullPage: !name.endsWith('menu'), mask: [page.locator('code'), page.locator('pre')] });
}
async function cancelDialog(page: Page) { await page.keyboard.press('Escape'); await expect(page.getByRole('dialog')).toHaveCount(0); }

test('real Electron: rail account menu and reachable working security controls', async ({ browser }, info) => {
  if (!appRoot) throw new Error('Set POCKYMOE_ELECTRON_APP_ROOT to an independent built desktop checkout');
  mkdirSync(evidence, { recursive: true });
  const f = await fixture(browser, info); let app;
  try {
    const owner = await f.account('shell-owner');
    const offline = await f.api('/relay/devices', 'POST', { name: 'Offline laptop' }, owner.cookie);
    expect(offline.status).toBe(200); f.secrets.push(offline.data.token);
    const req = createRequire(resolve(appRoot, 'package.json'));
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => ['PATH', 'HOME', 'TMPDIR', 'USER', 'LOGNAME'].includes(key))) as Record<string, string>;
    Object.assign(env, { POCKYMOE_DESKTOP_E2E: '1', POCKYMOE_DESKTOP_USER_DATA: join(f.root, 'electron-profile'), POCKYMOE_DESKTOP_WEB_ORIGIN: f.base, POCKYMOE_DESKTOP_RELAY_ORIGIN: f.base });
    app = await electron.launch({ executablePath: req('electron'), args: ['--use-mock-keychain', resolve(appRoot, 'out/main/index.js')], env });
    await app.context().addInitScript(id => { localStorage.setItem('remote-codex.locale', 'en'); localStorage.setItem(`pockymoe.onboarding.v1:${JSON.stringify([location.origin, `relay:${id}`])}`, JSON.stringify({ welcomeDismissed: true, completed: [], resume: {} })); }, owner.userId);
    await expect.poll(() => app!.windows().some(page => page.url().startsWith(f.base))).toBe(true);
    const page = app.windows().find(page => page.url().startsWith(f.base))!;
    page.setDefaultTimeout(15000);
    await (await app.browserWindow(page)).evaluate(window => window.setBounds({ width: 1440, height: 900 }));
    await page.goto(f.base + '/relay-portal?returnTo=%2F');
    try {
      await page.getByLabel('Email or username', { exact: true }).fill(owner.username);
      await page.getByLabel('Password', { exact: true }).fill(owner.password);
      await page.getByRole('button', { name: 'Sign in', exact: true }).click();
      await expect(page.locator('[data-desktop-surface]')).toBeVisible();
    } catch { await page.goto('about:blank'); throw new Error('Isolated Electron sign-in failed'); }
    expect(await page.evaluate(async () => (await window.pockymoeHost!.capabilities()).surface)).toBe('desktop');
    const rail = page.locator('.desktop-map-scene:not([hidden]) .matter-rail');
    const avatar = rail.locator('[data-account-rail]');
    await expect(avatar).toBeVisible();
    await expect(rail.locator('[data-icon="settings"]')).toHaveCount(0);
    expect((await avatar.boundingBox())!.height).toBe(40);
    await avatar.click();
    const menu = page.getByRole('menu');
    expect(await menu.evaluate(element => element.parentElement === document.body)).toBe(true);
    expect((await menu.boundingBox())!.y + (await menu.boundingBox())!.height).toBeCloseTo((await avatar.boundingBox())!.y - 8, 0);
    await expect(page.getByRole('menuitem', { name: 'Account settings' })).toBeFocused();
    expect((await menu.getByRole('menuitem').allTextContents()).map(text => text.trim())).toEqual(['Account settings', 'Settings', 'Device management', 'Tutorial', 'Log out']);
    await page.keyboard.press('End'); await expect(page.getByRole('menuitem', { name: 'Log out' })).toBeFocused();
    await page.keyboard.press('Escape'); await expect(avatar).toBeFocused();
    await avatar.click(); await page.getByRole('menuitem', { name: 'Tutorial', exact: true }).click();
    await expect(page.getByRole('dialog', { name: 'Tutorial', exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Close tutorial', exact: true }).click();
    await avatar.click(); await page.getByRole('menuitem', { name: 'Settings', exact: true }).click();
    await expect(page.getByTestId('settingsDialog')).toBeVisible(); await cancelDialog(page);
    await avatar.click(); await page.getByRole('menuitem', { name: 'Account settings' }).click();
    await expect(page.locator('.desktop-workspaces-scroll [data-page="account"]')).toBeVisible();
    await expect(page.locator('[data-page="account"] .product-topbar')).toHaveCount(0);
    const scroll = page.locator('.desktop-workspaces-scroll');
    const before = await scroll.evaluate(element => element.scrollTop);
    await page.mouse.move(1100, 700); await page.mouse.wheel(0, 1100);
    await expect.poll(() => scroll.evaluate(element => element.scrollTop)).toBeGreaterThan(before);
    const changedPassword = f.password + 'new'; f.secrets.push(changedPassword);
    await page.getByRole('button', { name: 'Change password', exact: true }).click();
    await page.getByLabel('New password', { exact: true }).fill(changedPassword);
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect.poll(async () => await page.getByRole('dialog', { name: UNLOCK_DIALOG }).isVisible() || await page.getByText('Password changed.', { exact: true }).isVisible()).toBe(true);
    if (await page.getByRole('dialog', { name: UNLOCK_DIALOG }).isVisible()) await unlock(page, owner.password);
    await expect(page.getByText('Password changed.', { exact: true })).toBeVisible();
    console.log('Electron password changed');
    await page.getByRole('button', { name: 'Regenerate recovery code', exact: true }).click();
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    console.log('Electron recovery confirmation visible');
    const recovery = await readRecoveryCode(page); f.secrets.push(recovery);
    await confirmRecoveryCode(page, recovery);
    await page.getByRole('button', { name: 'Start over', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Cancel', exact: true })).toBeFocused();
    await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    // This fixture models an OAuth-established account by changing only the
    // disposable server's auth mode, preserving its real encrypted vault.
    await f.sql('c.execute("UPDATE relay_account_auth SET oauth_only=1,auth_hash=NULL,auth_salt=NULL WHERE user_id=?", (p["user"],))', { user: owner.userId });
    console.log('Electron OAuth fixture ready');
    await page.reload();
    await expect(page.getByRole('heading', { name: 'Sign-in and security' })).toBeVisible();
    await page.getByRole('button', { name: 'Why it is unavailable', exact: true }).first().click();
    await expect(page.getByText(/OAuth accounts have no sign-in password/)).toBeVisible();
    await page.keyboard.press('Escape');
    await page.getByRole('button', { name: 'Change password', exact: true }).click();
    await page.getByLabel('New password', { exact: true }).fill(changedPassword + 'oauth');
    await page.getByRole('button', { name: 'Continue', exact: true }).click();
    await expect.poll(async () => await page.getByRole('dialog', { name: UNLOCK_DIALOG }).isVisible() || await page.getByText('Password changed.', { exact: true }).isVisible()).toBe(true);
    if (await page.getByRole('dialog', { name: UNLOCK_DIALOG }).isVisible()) await unlock(page, changedPassword);
    await expect(page.getByText('Password changed.', { exact: true })).toBeVisible();
    for (const pack of ['classic', 'plum-pocket', 'terminal']) for (const mode of ['light', 'dark']) {
      await page.evaluate(({pack,mode}) => { localStorage.setItem('pockymoe-theme-preset', pack); localStorage.setItem('remote-codex-theme-mode', mode); }, {pack,mode});
      await page.reload();
      await expect(page.locator('html')).toHaveAttribute('data-theme-preset', pack);
      await expect(page.getByRole('heading', { name: 'Danger zone' })).toBeAttached();
      await capture(page, `${pack}-${mode}-desktop-account`);
      await scroll.evaluate(element => element.scrollTop = element.scrollHeight);
      await capture(page, `${pack}-${mode}-desktop-account-bottom`);
      const accountAvatar = page.locator('.desktop-workspaces-shell .matter-rail [data-account-rail]');
      await accountAvatar.click(); await capture(page, `${pack}-${mode}-desktop-menu`); await page.keyboard.press('Escape');
      await accountAvatar.click(); await page.getByRole('menuitem', { name: 'Device management', exact: true }).click();
      await expect(page.getByTestId('desktop-devices')).toBeVisible();
      await capture(page, `${pack}-${mode}-desktop-devices`);
      await page.getByRole('tab', { name: /Sharing/ }).click();
      await expect(page.locator('#devices-page-panel-shared')).toBeVisible();
      await capture(page, `${pack}-${mode}-desktop-shared`);
      await page.locator('.desktop-map-scene:not([hidden]) [data-account-rail]').click();
      await page.getByRole('menuitem', { name: 'Account settings' }).click();
    }
    writeFileSync(`${evidence}/electron-results.json`, JSON.stringify({ realElectron: true, scrollByWheel: true, passwordChanged: true, recoveryRegenerated: true, oauthUnlockPasswordChanged: true, menuPortal: true, menuGap: 8, cancelFocused: true, tourHub: true }, null, 2));
  } finally { if (app) { const running = app; await Promise.race([running.close(), new Promise<void>(done => { const timer = setTimeout(() => { if (running.process().exitCode === null) running.process().kill('SIGKILL'); done(); }, 5000); timer.unref(); })]); } await f.close(); }
});

test('browser routes: PC queries, signed-out portal, retained phone and mobile-host page', async ({ browser }, info) => {
  const f = await fixture(browser, info);
  try {
    const owner = await f.account('route-owner'); const page = owner.page;
    const offline = await f.api('/relay/devices', 'POST', { name: 'Offline laptop' }, owner.cookie);
    expect(offline.status).toBe(200);
    for (const query of ['', '?tab=shared', '?tab=shared&dir=out']) {
      await page.goto(f.base + '/relay-devices' + query);
      await expect(page).toHaveURL(f.base + '/desktop/devices' + query);
      await expect(page.getByTestId('desktop-devices')).toBeVisible();
      await expect(page.locator('.product-title')).toHaveCount(0);
      await expect(page.getByRole('tab', { name: query ? /Sharing/ : /^Devices/ })).toHaveAttribute('aria-selected', 'true');
    }
    await page.goto(f.base + `/relay-devices?shareDevice=${offline.data.device.id}`);
    await expect(page.getByRole('dialog')).toBeVisible(); await cancelDialog(page);
    const signedOut = await f.context(); await signedOut.page.goto(f.base + '/relay-devices?tab=shared&dir=out');
    await expect(signedOut.page).toHaveURL(/\/relay-portal\?/);
    await expect(signedOut.page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    // Start a new document after setting the phone width, then resize while
    // mounted: neither the route nor its helper switches destinations.
    await page.setViewportSize({width:390,height:844});
    await page.goto(f.base + '/relay-devices?tab=shared&dir=out');
    await expect(page.locator('.devices-page-tabs')).toBeVisible();
    await expect(page.getByTestId('desktop-devices')).toHaveCount(0);
    await page.setViewportSize({width:1280,height:900});
    await expect(page).toHaveURL(f.base + '/relay-devices?tab=shared&dir=out');
    await expect(page.locator('.devices-page-tabs')).toBeVisible();
    for (const pack of ['classic','plum-pocket','terminal']) for (const mode of ['light','dark']) {
      await page.setViewportSize({width:390,height:844});
      await page.evaluate(({pack,mode})=>{localStorage.setItem('pockymoe-theme-preset',pack);localStorage.setItem('remote-codex-theme-mode',mode);},{pack,mode});
      await page.goto(f.base+'/relay-account');
      await expect(page.getByRole('heading',{name:'Sign-in and security'})).toBeAttached();
      await capture(page,`${pack}-${mode}-phone-account`);
      await page.locator('.product-topbar').getByRole('button',{name:/Relay account menu/}).click();
      await capture(page,`${pack}-${mode}-phone-menu`); await page.keyboard.press('Escape');
      await page.goto(f.base+'/relay-devices'); await expect(page.locator('.devices-page-tabs')).toBeVisible();
      await capture(page,`${pack}-${mode}-phone-devices`);
      await page.getByRole('tab',{name:/Sharing/}).click();
      await expect(page.getByRole('tab',{name:/Sharing/})).toHaveAttribute('aria-selected','true');
      await expect(page.locator('#devices-page-panel-shared')).toBeVisible();
      await capture(page,`${pack}-${mode}-phone-shared`);
      expect(await page.evaluate(()=>document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    }
    const mobile = { page, context: owner.context };
    await mobile.context.addInitScript(() => { (window as any).pockymoeHost = { capabilities: async () => ({surface:'mobile',platform:'ios',haptics:false}), onCommand:()=>()=>{}, commandResult:async()=>{}, window:{setTitle:async()=>{}}, settings:{get:async()=>({webOrigin:location.origin,relayOrigin:location.origin})}, activity:{sync:async()=>{}}, appearance:{sync:async()=>{}} }; });
    await mobile.page.setViewportSize({width:1280,height:900});
    await mobile.page.goto(f.base+'/relay-devices?tab=shared&dir=out');
    await expect(mobile.page.locator('.devices-page-tabs')).toBeVisible();
    await expect(mobile.page.getByTestId('desktop-devices')).toHaveCount(0);
    writeFileSync(`${evidence}/route-results.json`,JSON.stringify({pcRedirect:true,oldQueriesPreserved:true,shareDeviceOpened:true,signedOutPortal:true,phoneRetained:true,resizeStable:true,mobileHostRetained:true},null,2));
  } finally { await f.close(); }
});
