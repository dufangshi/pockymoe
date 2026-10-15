import { test, expect, type Page } from '@playwright/test';
import { createHash, createPublicKey, generateKeyPairSync, verify } from 'node:crypto';
import { fixture, browserRequest, ok, unlock, UNLOCK_DIALOG, type Account, type Device } from './helpers/account-key-fixture';

test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

function certificateValid(descriptor: any, device: Device, owner: Account) {
  const c = descriptor.certificate;
  if (!c || c.version !== 1 || c.deviceId !== device.id || c.accountKeyId !== owner.accountKeyId || c.identityKey !== descriptor.identityKey) return false;
  try {
    const input = `pockymoe-acct-device-v1\n${c.accountKeyId}\n${c.deviceId}\n${c.identityKey}\n${c.createdAt}`;
    return verify('sha256', Buffer.from(input), {
      key: createPublicKey({ key: Buffer.from(owner.publicKey, 'base64url'), format: 'der', type: 'spki' }),
      dsaEncoding: 'ieee-p1363',
    }, Buffer.from(c.signature, 'base64url'));
  } catch { return false; }
}

async function protectAll(page: Page, password: string, label: string) {
  const action = page.getByRole('button', { name: 'Protect all devices', exact: true });
  await expect.soft(action, `${label}_ACTION: Devices must offer the approved bulk protection action`).toBeVisible({ timeout: 3_000 });
  const offered = await action.isVisible();
  // No API/local-command substitute: missing product UI remains a soft failure,
  // and the following state/certificate assertions still run independently.
  if (offered) {
    await action.click();
    const confirmation = page.getByRole('dialog', { name: 'Protect all devices', exact: true });
    await expect(confirmation, `${label}_CONFIRMATION: bulk protection first explains the trust decision`).toBeVisible();
    await expect(confirmation.getByText(/Remote protection trusts the relay at the moment of binding\./),
      `${label}_TRADEOFF: the user sees that the relay is trusted during the first bind`).toBeVisible();
    await confirmation.getByRole('button', { name: 'Protect devices', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: UNLOCK_DIALOG });
    await expect.soft(dialog, `${label}_UNLOCK: a locked page must request one account unlock for the whole batch`).toBeVisible({ timeout: 5_000 });
    if (await dialog.isVisible()) await unlock(page, password);
  }
  return offered;
}

test('one-click protection certifies two unbound devices and detects a relay-substituted account key', async ({ browser }, info) => {
  test.skip(info.project.name !== 'desktop-chromium', 'One-click protocol coverage runs on desktop only');
  test.setTimeout(180_000);
  const f = await fixture(browser, info);
  try {
    const owner = await f.account('one-click-owner');
    const bound = await f.device(owner, 1);
    const before = await f.bind(owner, bound);
    const devices = [await f.device(owner, 2), await f.device(owner, 3)];
    for (const device of devices) {
      expect((await f.api(device.api + `/threads/${device.thread}`, 'GET', undefined, owner.cookie)).status,
        'Each new runtime starts unbound and relay-trusted').toBe(200);
    }
    await owner.page.addInitScript(() => {
      (window as any).__bulkUnlockSubmissions = 0;
      document.addEventListener('submit', event => {
        const form = event.target;
        if (form instanceof HTMLFormElement && /Unlock this browser|Confirm with your password/.test(form.closest('[role="dialog"]')?.textContent ?? '') && form.querySelector('input[type="password"]')) {
          (window as any).__bulkUnlockSubmissions++;
        }
      }, true);
    });
    const bindTargets: string[] = [], bindStatuses: number[] = [];
    owner.page.on('request', request => {
      const match = /^\/relay\/devices\/([^/]+)\/api\/transport\/bind$/.exec(new URL(request.url()).pathname);
      // Record public routing facts only; the bind body contains a secret.
      if (match && request.method() === 'POST') bindTargets.push(match[1]);
    });
    owner.page.on('response', response => {
      if (new URL(response.url()).pathname.endsWith('/api/transport/bind')) bindStatuses.push(response.status());
    });
    // A full navigation clears the account-key unlock window. The existing
    // browser certificate still permits ordinary device reads.
    await owner.page.goto(f.base + '/relay-devices');
    const row = (device: Device) => owner.page.locator('article').filter({ has: owner.page.getByText(`Signed device ${device.db.match(/device-(\d+)/)![1]}`, { exact: true }) });
    for (const device of devices) await expect(row(device).getByRole('button', { name: 'Not protected', exact: true })).toBeVisible();
    const offered = await protectAll(owner.page, owner.password, 'ONE_CLICK');
    await expect.configure({ soft: true }).poll(async () => (await Promise.all(devices.map(device => row(device).getByRole('button', { name: 'Protected', exact: true }).isVisible()))).every(Boolean),
      { timeout: offered ? 30_000 : 100, message: 'ONE_CLICK_PROTECTED: both initially unbound owned devices must complete enrollment automatically' }).toBe(true);
    expect.soft(await owner.page.evaluate(() => (window as any).__bulkUnlockSubmissions),
      'ONE_CLICK_SINGLE_UNLOCK: one password submission must protect both devices').toBe(1);
    expect.soft(await owner.page.getByRole('dialog', { name: UNLOCK_DIALOG }).count(),
      'ONE_CLICK_NO_EXTRA_UNLOCK: no second unlock may remain queued').toBe(0);
    for (const device of devices) {
      const descriptor = ok(await f.api(device.api + '/transport/key?challenge=one-click-protection', 'GET', undefined, owner.cookie), 'Bulk-protected descriptor');
      expect.soft(certificateValid(descriptor, device, owner),
        `ONE_CLICK_CERTIFICATE: ${device.id} must present an independently verified owner account certificate matching its identity`).toBe(true);
      expect.soft(descriptor.trust, 'ONE_CLICK_STRICT: bulk protection must pin the account and enable strict mode').toMatchObject({ bound: true, strict: true, accountKeyId: owner.accountKeyId });
      const plaintext = await f.api(device.api + `/threads/${device.thread}`, 'GET', undefined, owner.cookie);
      expect.soft(plaintext.status, 'ONE_CLICK_PLAINTEXT: initially allowed relay-only reads must be refused after protection').toBe(403);
      expect.soft(plaintext.data?.code, 'ONE_CLICK_PLAINTEXT_CODE: refusal must originate from strict device transport').toBe('device_strict_mode');
      expect.soft(bindTargets.includes(device.id), 'ONE_CLICK_BIND_REQUEST: each initially unbound device must receive the real UI bind request').toBe(true);
      f.events.push(`one-click device=${device.id} validCertificate=${certificateValid(descriptor, device, owner)} plaintext=${plaintext.status}/${plaintext.data?.code ?? 'none'}`);
    }
    expect.soft(bindTargets.includes(bound.id), 'ONE_CLICK_BOUND_UNTOUCHED: an already bound device must not receive a bind request').toBe(false);
    const after = ok(await browserRequest(owner.page, bound.api + '/transport/trust'), 'Original bound device remains accessible');
    expect(after.certificate, 'Bulk protection preserves the already bound device certificate exactly').toEqual(before.certificate);
    expect(after).toMatchObject({ bound: true, strict: true, accountKeyId: owner.accountKeyId });

    // The attack happens when the next unbound device actually receives its
    // first bind, independent of browser routing or Service Worker timing.
    // Only this fixture's temporary device DB is modified. The genuine pending
    // enrollment secret is retained, so the Web must detect the wrong account.
    const attacked = await f.device(owner, 4);
    const foreignKey = generateKeyPairSync('ec', { namedCurve: 'prime256v1' }).publicKey.export({ format: 'der', type: 'spki' });
    const foreignPublicKey = foreignKey.toString('base64url');
    const foreignAccountKeyId = createHash('sha256').update(foreignKey).digest('base64url');
    await f.sql(`
c.execute("CREATE TABLE e2e_bind_substitution(account_key_id TEXT,account_public_key TEXT)")
c.execute("INSERT INTO e2e_bind_substitution VALUES(?,?)",(p["accountKeyId"],p["publicKey"]))
c.executescript('''CREATE TRIGGER e2e_substitute_first_bind AFTER UPDATE OF account_key_id ON device_owner_trust
WHEN OLD.account_key_id IS NULL AND NEW.account_key_id IS NOT NULL
BEGIN
  UPDATE device_owner_trust SET account_key_id=(SELECT account_key_id FROM e2e_bind_substitution),
    account_public_key=(SELECT account_public_key FROM e2e_bind_substitution) WHERE singleton=1;
END;''')`, { db: attacked.db, accountKeyId: foreignAccountKeyId, publicKey: foreignPublicKey });
    await owner.page.goto(f.base + '/relay-devices');
    await expect(row(attacked).getByRole('button', { name: 'Not protected', exact: true })).toBeVisible();
    const attackOffered = await protectAll(owner.page, owner.password, 'ONE_CLICK_MISMATCH');
    await expect.configure({ soft: true }).poll(async () => await f.sql('print(json.dumps(c.execute("SELECT account_key_id FROM device_owner_trust WHERE singleton=1").fetchone()[0]))', { db: attacked.db }),
      { timeout: attackOffered ? 15_000 : 100, message: 'ONE_CLICK_MISMATCH_INJECTED: fixture must substitute the account key during the actual first UI bind' }).toBe(foreignAccountKeyId);
    const mismatch = row(attacked).getByRole('button', { name: 'Bound to another account', exact: true });
    await expect.soft(mismatch, 'ONE_CLICK_MISMATCH_UI: Web must explicitly report a device bound to the wrong account key').toBeVisible({ timeout: attackOffered ? 15_000 : 100 });
    await expect(row(attacked).getByRole('button', { name: 'Protected', exact: true }), 'Substituted account must never be labeled Protected').toHaveCount(0);
    const attackedDescriptor = ok(await f.api(attacked.api + '/transport/key?challenge=one-click-mismatch', 'GET', undefined, owner.cookie), 'Substituted device descriptor');
    expect(certificateValid(attackedDescriptor, attacked, owner), 'A wrong-account device must not acquire an owner certificate').toBe(false);
    f.events.push(`one-click offered=${offered} bindStatuses=${bindStatuses.join(',') || 'none'} originalCertificateUnchanged=${JSON.stringify(after.certificate) === JSON.stringify(before.certificate)} mismatchOffered=${attackOffered} mismatchVisible=${await mismatch.isVisible()} substitutedAccount=${attackedDescriptor.trust?.accountKeyId === foreignAccountKeyId}`);
  } finally { await f.close(); }
});
