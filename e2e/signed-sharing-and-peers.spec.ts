import { test, expect } from '@playwright/test';
import { fixture, browserRequest, ok, sendPrompt, sockets, screenshot, shareDialog, approveGrant, unlock, type Device } from './helpers/account-key-fixture';

// No auth material enters traces, videos or automatic failure screenshots.
test.use({ trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

test('signed sharing, invite pins, revocation and account-certified peers', async ({ browser }, info) => {
  test.setTimeout(240_000);
  const mobile = info.project.name === 'mobile-chromium';
  const f = await fixture(browser, info);
  try {
    const owner = await f.account('signed-owner'), recipient = await f.account('signed-recipient');
    expect(recipient.accountKeyId).not.toBe(owner.accountKeyId);
    const d1 = await f.device(owner, 1);
    await f.bind(owner, d1);
    await owner.page.goto(d1.url);
    await sendPrompt(owner.page, 'SIGNED_HISTORY');
    const dialog = await shareDialog(owner.page, d1.url);
    await dialog.getByPlaceholder('username or email').fill('signed-recipient');
    await expect(dialog.getByRole('radio', { name: 'View only', exact: true })).toBeChecked();
    await dialog.getByRole('button', { name: 'Share this thread', exact: true }).click();
    await approveGrant(owner.page, owner.password, true);
    await expect(dialog.getByText('signed-recipient', { exact: true })).toBeVisible();
    const portal = ok(await f.api('/relay/portal', 'GET', undefined, owner.cookie), 'Owner shares');
    const share = portal.sharedByMe.find((row: any) => row.threadId === d1.thread && row.targetUserId === recipient.userId);
    expect(!!share, 'Real share dialog created relay row').toBe(true);
    const grantResult = await browserRequest(owner.page, d1.api + '/transport/grants');
    const grants = ok(grantResult, 'Owner grant listing').grants;
    const grant = grants.find((row: any) => row.relayRef.kind === 'share' && row.relayRef.id === share.id);
    expect.soft(!!grant, 'SIGNED_SHARE_GRANT: share dialog must create a device grant, not only a relay row').toBe(true);
    if (!grant) throw new Error('SIGNED_SHARE_GRANT blocks recipient and revocation scenarios; no API-created grant substitutes for the real dialog');
    expect(grant).toMatchObject({ version: 1, scope: 'thread', threadId: d1.thread, threadAccess: 'read', grantee: { userId: recipient.userId } });
    expect(grant.grantee).toMatchObject({ accountKeyId: recipient.accountKeyId, accountPublicKey: recipient.publicKey });
    await expect(dialog.locator('code').filter({ hasText: recipient.code })).toHaveText(recipient.code);
    await expect(dialog.getByText(/Unverified/)).toBeVisible();
    await dialog.getByRole('button', { name: 'I compared the safety code', exact: true }).click();
    await expect(dialog.getByText(/· Verified/)).toBeVisible();
    if (mobile) await screenshot(owner.page, info, 'v2-signed-share-dialog');
    await owner.context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: f.base });
    await dialog.getByRole('button', { name: 'Copy invite link', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Invite link copied', exact: true })).toBeVisible();
    const invite = await owner.page.evaluate(() => navigator.clipboard.readText());
    const fragment = new URLSearchParams(new URL(invite).hash.slice(1));
    expect(new URL(invite).pathname).toBe('/shared');
    expect(fragment.get('device')).toBe(d1.id); expect(fragment.get('thread')).toBe(d1.thread);
    expect(fragment.get('v')).toBe('2'); expect(fragment.get('owner')).toBe(owner.accountKeyId);
    await dialog.getByRole('button', { name: 'Close dialog', exact: true }).click();

    // Recipient has a separate account key and browser certificate.
    const recipientChannels = sockets(recipient.page);
    await recipient.page.goto(d1.url);
    await expect.soft(recipient.page.getByText('SIGNED_HISTORY', { exact: true }), 'RECIPIENT_READ: signed read grant must resume history after recipient unlock').toBeVisible({ timeout: 5_000 });
    // Keep diagnosis visible, then permit the remaining independent checks.
    if (!(await recipient.page.getByText('SIGNED_HISTORY', { exact: true }).isVisible())) await recipient.page.reload();
    await expect(recipient.page.getByText('SIGNED_HISTORY', { exact: true })).toBeVisible();
    await expect.poll(() => recipientChannels.some(c => c.frames.some(frame => frame.includes('"encrypted"')))).toBe(true);
    await expect(recipient.page.getByRole('button', { name: /Encrypted shared connection · owner account trusted on first use$/ }).first()).toBeVisible();
    const read = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}`);
    expect(read.status, `Recipient signed read (code ${read.data?.code}: ${read.data?.message})`).toBe(200); expect(read.data.thread.id).toBe(d1.thread);

    // Fresh browser opens the owner-copied invitation before its first handshake.
    const fresh = await f.login(recipient);
    let invalidInviteChallenge = false;
    fresh.page.on('response', response => {
      if (response.status() === 400 && new URL(response.url()).pathname.endsWith('/transport/key') &&
        new URL(response.url()).searchParams.get('challenge')?.includes('_')) invalidInviteChallenge = true;
    });
    await fresh.page.goto(invite);
    await expect(fresh.page.getByRole('heading', { name: 'Open shared access', exact: true })).toBeVisible();
    await expect(fresh.page.getByRole('button', { name: 'Confirm owner account key', exact: true })).toBeVisible();
    if (mobile) await screenshot(fresh.page, info, 'v2-shared-invite');
    await fresh.page.getByRole('button', { name: 'Confirm owner account key', exact: true }).click();
    await expect.poll(async () => fresh.page.url() === d1.url || await fresh.page.getByRole('alert').isVisible()).toBe(true);
    if (invalidInviteChallenge) {
      expect.soft(false, 'INVITE_CHALLENGE: sharedDeviceTrust generates base64url challenges containing underscore; Supervisor tunnel accepts only alphanumeric/hyphen and returns 400 Invalid key challenge. Invitation pin was saved; navigate to the destination to verify it using the normal UUID-challenge handshake.').toBe(true);
      await fresh.page.goto(d1.url);
    }
    await expect(fresh.page.getByText('SIGNED_HISTORY', { exact: true })).toBeVisible();
    const pinned = fresh.page.getByRole('button', { name: /Encrypted shared connection · invited owner account verified$/ }).first();
    await expect.soft(pinned, 'INVITE_PIN: v2 invite must pin the owner account, with a verified device certificate').toBeVisible({ timeout: 5_000 });
    if (mobile) return;

    const tampered = await f.login(recipient);
    const bad = new URL(invite); const badFragment = new URLSearchParams(bad.hash.slice(1));
    badFragment.set('owner', 'A'.repeat(43)); bad.hash = badFragment.toString();
    let invalidTamperedChallenge = false;
    tampered.page.on('response', response => {
      if (response.status() === 400 && new URL(response.url()).pathname.endsWith('/transport/key') &&
        new URL(response.url()).searchParams.get('challenge')?.includes('_')) invalidTamperedChallenge = true;
    });
    await tampered.page.goto(bad.href);
    await tampered.page.getByRole('button', { name: 'Confirm owner account key', exact: true }).click();
    await expect.soft(tampered.page.getByRole('alert'), 'INVITE_MISMATCH: tampered owner account must block the connection').toContainText(/does not match|certificate is missing or invalid/, { timeout: 5_000 });
    if (invalidTamperedChallenge) {
      expect.soft(false, 'INVITE_CHALLENGE_TAMPERED: invalid underscore challenge obscures the owner-pin mismatch; verify the saved tampered pin on the normal destination handshake.').toBe(true);
      await tampered.page.goto(d1.url);
      await expect(tampered.page.locator('[data-encryption="identity-changed"]').first()).toBeVisible();
    }
    expect(await tampered.page.getByRole('button', { name: 'Send Prompt', exact: true }).isEnabled().catch(() => false)).toBe(false);
    expect(await tampered.page.getByText('SIGNED_HISTORY', { exact: true }).count()).toBe(0);

    // Defense in depth: relay refuses a read-only prompt before device dispatch.
    const relayDenied = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}/prompt`, 'POST', { prompt: 'READ_ONLY_RELAY_REFUSAL' });
    expect(relayDenied.status).toBe(403);
    f.events.push(`read-only relay denial code=${relayDenied.data.code}`);
    // Compromise only the relay row. A newly accepted relay write still must be
    // refused by the unchanged read grant on the bound device.
    await f.relayRow(share.id, 'control');
    const deviceDenied = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}/prompt`, 'POST', { prompt: 'RELAY_WIDENED_REFUSAL' });
    expect.soft(deviceDenied.status, 'RELAY_WIDENING: bound device must refuse relay-only control').toBe(403);
    expect.soft(deviceDenied.data.code, 'RELAY_WIDENING: refusal must originate from the device grant ACL').toBe('grant_denied');
    await owner.page.goto(f.base + '/relay-devices');
    await owner.page.getByRole('tab', { name: /^Threads by me/ }).click();
    await expect.soft(owner.page.getByText('Device permissions differ from this share', { exact: true }),
      'RECONCILIATION: Shared access must flag relay/device permission mismatch').toBeVisible({ timeout: 5_000 });
    await f.relayRow(share.id, 'read');
    const editing = await shareDialog(owner.page, d1.url);
    await editing.getByRole('button', { name: 'Edit permissions for signed-recipient', exact: true }).click();
    await editing.getByRole('radio', { name: 'Collaborator', exact: true }).check();
    await editing.getByRole('button', { name: 'Save permissions', exact: true }).click();
    await approveGrant(owner.page, owner.password);
    await expect.poll(async () => ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Updated grants').grants.find((g: any) => g.grantId === grant.grantId)?.threadAccess).toBe('control');
    await editing.getByRole('button', { name: 'Close dialog', exact: true }).click();
    await recipient.page.reload();
    await expect(recipient.page.getByRole('button', { name: 'Send Prompt', exact: true })).toBeEnabled();
    await expect.poll(() => recipientChannels.filter(c => !c.closed).some(c => c.frames.some(frame => frame.includes('"encrypted"')))).toBe(true);
    const active = recipientChannels.filter(c => !c.closed).at(-1)!;
    const beforeReply = active.frames.length;
    await sendPrompt(recipient.page, 'RECIPIENT_CONTROL_REPLY');
    expect(active.frames.slice(beforeReply).some(frame => frame.includes('"encrypted"')), 'Recipient control reply arrives over encrypted WebSocket').toBe(true);
    expect(recipientChannels.some(c => c.frames.some(frame => /SIGNED_HISTORY|RECIPIENT_CONTROL_REPLY/.test(frame))), 'Relay cannot read history or control replies').toBe(false);

    const revoking = await shareDialog(owner.page, d1.url);
    const revokedAt = Date.now();
    await revoking.getByRole('button', { name: 'Revoke', exact: true }).click();
    await expect(revoking.getByText('signed-recipient', { exact: true })).toHaveCount(0);
    expect(ok(await browserRequest(owner.page, d1.api + '/transport/grants'), 'Revoked grants').grants.some((g: any) => g.grantId === grant.grantId)).toBe(false);
    // Restore only the relay ACL so neither HTTP denial nor WS shutdown can be
    // explained solely by the relay row's disappearance.
    await f.relayRow(share.id, 'restore');
    const revokedRead = await browserRequest(recipient.page, d1.api + `/threads/${d1.thread}`);
    f.events.push(`revoked next read status=${revokedRead.status} code=${revokedRead.data.code}`);
    expect.soft(revokedRead.status, 'REVOCATION_READ: restored relay ACL cannot restore device access').toBeGreaterThanOrEqual(400);
    expect.soft(['grant_denied', 'browser_registration_required', 'browser_revoked', 'account_mismatch'].includes(revokedRead.data.code),
      `REVOCATION_CODE: next read returned ${revokedRead.data.code}; v2 grant deletion drops recipient browser registration`).toBe(true);
    await expect.configure({ soft: true }).poll(() => active.closed, { timeout: Math.max(100, 5_000 - (Date.now() - revokedAt)), message: 'REVOCATION_WS: existing secure channel must close within ~5 seconds' }).toBe(true);
    expect.soft(active.closedAt !== undefined && active.closedAt - revokedAt <= 5_000,
      'REVOCATION_WS_DEADLINE: channel close timestamp must be within ~5 seconds of the UI revoke').toBe(true);
    f.events.push(`revoked channel close elapsedMs=${active.closedAt === undefined ? 'not-closed' : active.closedAt - revokedAt}`);
    await revoking.getByRole('button', { name: 'Close dialog', exact: true }).click();
    const afterRevoke = active.frames.length;
    await sendPrompt(owner.page, 'AFTER_REVOKE_PRIVATE');
    expect.soft(active.frames.slice(afterRevoke).some(frame => frame.includes('"encrypted"')), 'REVOCATION_EVENTS: revoked open channel must not receive owner events').toBe(false);
    expect(await recipient.page.getByText('AFTER_REVOKE_PRIVATE', { exact: true }).count()).toBe(0);

    // Peer CLI uses only each disposable Supervisor's generated machine config,
    // with --from tying the send to a real local thread (same as live peer E2E).
    const d2 = await f.device(owner, 2);
    await f.bind(owner, d2);
    for (const device of [d1, d2]) expect((await f.cli(['--cli-config', device.cli, 'device', 'access', 'on'])).code, 'Enable peer access locally').toBe(0);
    const args = (source: Device, target: Device, subject: string) => ['--cli-config', source.cli, '--from', source.thread,
      'thread', 'send', `${target.id}/${target.thread}`, '--kind', 'status', '--subject', subject, '--text', subject];
    const sent = await f.cli(args(d1, d2, 'CERTIFIED_PEER_DELIVERY'));
    expect.soft(sent.code, 'PEER_CERTIFICATE: devices bound to the same account must permit a local CLI peer send').toBe(0);
    expect.soft(sent.data?.delivery, 'PEER_DELIVERY: certified message is delivered as passive inbox mail').toBe('inbox');
    const inbox = await f.cli(['--cli-config', d2.cli, 'inbox', '--thread', d2.thread]);
    expect(inbox.code).toBe(0);
    const delivered = inbox.data.messages.find((m: any) => m.subject === 'CERTIFIED_PEER_DELIVERY');
    expect.soft(delivered, 'PEER_ATTRIBUTION: target inbox records device and source thread').toMatchObject({ fromDeviceId: d1.id, fromThreadId: d1.thread, replyTo: `${d1.id}/${d1.thread}` });
    const d3 = await f.device(owner, 3);
    await f.bind(owner, d3);
    expect((await f.cli(['--cli-config', d3.cli, 'device', 'access', 'on'])).code).toBe(0);
    await owner.page.goto(f.base + '/relay-devices');
    // Save registration privately: browser routing cannot intercept a DELETE
    // handled by the Service Worker. Let the real UI revoke and delete D3.
    const registration = await f.sql(`
row=c.execute("SELECT id,owner_user_id,name,token,token_hash,token_preview,created_at FROM relay_devices WHERE id=?",(p["id"],)).fetchone()
assert row is not None
print(json.dumps(row))`, { id: d3.id });
    await owner.page.getByRole('button', { name: 'More actions for Signed device 3', exact: true }).click();
    await owner.page.getByRole('menuitem', { name: 'Remove device', exact: true }).click();
    const removal = owner.page.getByRole('dialog', { name: 'Remove device', exact: true });
    await removal.getByRole('button', { name: 'Remove device', exact: true }).click();
    await unlock(owner.page, owner.password);
    await expect(removal).toHaveCount(0);
    expect(await f.sql('print(json.dumps(c.execute("SELECT COUNT(*) FROM relay_devices WHERE id=?",(p["id"],)).fetchone()[0]))', { id: d3.id }),
      'Real UI removal deletes the relay registration').toBe(0);
    // Simulate a compromised relay restoring registration, without changing
    // the signed tombstone or either device's trust state. The same Supervisor
    // reconnects with its original token once the relay row is restored.
    await f.sql('c.execute("INSERT INTO relay_devices(id,owner_user_id,name,token,token_hash,token_preview,created_at) VALUES(?,?,?,?,?,?,?)",p["row"])', { row: registration });
    await expect.poll(async () => (await f.api(`/relay/devices/${d3.id}/presence`, 'GET', undefined, owner.cookie)).data?.connected,
      { timeout: 15_000, message: 'Restored D3 registration has a live, responding relay tunnel' }).toBe(true);
    const restored = ok(await f.api(d3.api + '/transport/key?challenge=revoked-peer-fixture', 'GET', undefined, owner.cookie), 'Restored target key handshake');
    expect(restored.certificate, 'Reachable revoked target still presents its account-signed certificate').toMatchObject({ deviceId: d3.id, accountKeyId: owner.accountKeyId });
    const revokedPeer = await f.cli(args(d1, d3, 'REVOKED_DEVICE_REFUSAL'));
    expect.soft(revokedPeer.code, 'PEER_REVOKED: account-revoked target cannot receive a peer send even when relay retains its tunnel').not.toBe(0);
    expect.soft(revokedPeer.output.includes('peer_target_unverified'), 'PEER_REVOKED_CERTIFICATE: D1 independently refuses the revoked target certificate').toBe(true);
    f.events.push(`revoked peer exit=${revokedPeer.code} peer_target_unverified=${revokedPeer.output.includes('peer_target_unverified')} device_not_found=${revokedPeer.output.includes('device_not_found')}`);
    const revocations = ok(await f.api('/relay/account/browsers', 'GET', undefined, owner.cookie), 'Signed device revocation').revocations;
    expect(revocations.some((r: any) => r.kind === 'device' && r.subjectId === d3.id)).toBe(true);
    expect((await f.cli(['bind', '--database', d2.db, '--reset', '--yes'])).code).toBe(0);
    const warm = await f.cli(args(d1, d2, 'RESET_TARGET_REFUSAL'));
    expect.soft(warm.code, 'PEER_RESET_CACHED: cached target descriptor must not authorize an unbound target after reset').not.toBe(0);
    expect.soft(warm.output.includes('peer_target_unverified'), 'PEER_RESET_CACHED: reset target must report peer_target_unverified').toBe(true);
    const unpairedCaller = await f.cli(args(d2, d1, 'UNPAIRED_CALLER_REFUSAL'));
    expect.soft(unpairedCaller.code, 'PEER_CALLER: strict target must refuse unsigned unbound caller').not.toBe(0);
    expect.soft(unpairedCaller.output.includes('peer_certificate_required'), 'PEER_CALLER: refusal must be peer_certificate_required').toBe(true);
    f.events.push(`peer certified=${sent.code} reset-cached=${warm.code} reset-cached-delivery=${warm.data?.delivery ?? 'none'} unpaired-caller=${unpairedCaller.code}`);
  } finally { await f.close(); }
});
