import { test, expect } from '@playwright/test';
import { mkdtemp, mkdir } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createECDH, randomBytes } from 'node:crypto';
import { createServer } from 'node:net';
import { DatabaseSync } from 'node:sqlite';
import { Fixture, ok } from './helpers/account-key-fixture';

// Use full Chromium's headless mode: headless-shell cannot display notifications.
test.use({ channel: 'chromium', trace: 'off', video: 'off', screenshot: 'off', actionTimeout: 15_000 });

test('account push registration, durable completion and background tab read state', async ({
  browser,
}, info) => {
  await mkdir(resolve('.local'), { recursive: true });
  const root = await mkdtemp(resolve('.local/push-e2e-'));
  const freePort = () =>
    new Promise<number>((done) => {
      const server = createServer();
      server.listen(0, '127.0.0.1', () => {
        const port = (server.address() as { port: number }).port;
        server.close(() => done(port));
      });
    });
  const relayPort = await freePort();
  const base = `http://localhost:${relayPort}`;
  class NotificationsFixture extends Fixture { override readonly base = base; }
  const fixture = new NotificationsFixture(root, browser, info);
  await mkdir(join(root, 'home'));
  try {
    fixture.start('relay', {
      PORT: String(relayPort),
      POCKYMOE_RELAY_DATA_DIR: join(root, 'relay'),
      POCKYMOE_RELAY_REGISTRATION_ENABLED: 'true',
      POCKYMOE_PUBLIC_BASE_URL: base,
      POCKYMOE_RELAY_WEB_DIST_DIR: resolve('apps/supervisor-web/dist'),
    });
    await expect.poll(() => fetch(base + '/healthz').then(r => r.status).catch(() => 0)).toBe(200);
    const owner = await fixture.account('notification-owner');
    const { context, page } = owner;
    const api = async (path: string, method = 'GET', body?: unknown) =>
      ok(await fixture.api(path, method, body, owner.cookie), `${method} ${path}`);
    const device = await fixture.device(owner, 1);
    await context.grantPermissions(['notifications'], { origin: base });
    // Chromium automation has no vendor push credentials. Mock only PushManager;
    // permission, worker registration, account UI and HTTP storage are real.
    const key = createECDH('prime256v1');
    key.generateKeys();
    await context.addInitScript(
      (subscription) => {
        let subscribed = false;
        const sub = {
          endpoint: subscription.endpoint,
          toJSON: () => subscription,
          unsubscribe: async () => {
            subscribed = false;
            return true;
          },
        };
        PushManager.prototype.getSubscription = async () =>
          subscribed ? (sub as PushSubscription) : null;
        PushManager.prototype.subscribe = async () => {
          subscribed = true;
          return sub as PushSubscription;
        };
      },
      {
        endpoint: 'https://fcm.googleapis.com/fcm/send/synthetic-e2e',
        keys: {
          p256dh: key.getPublicKey().toString('base64url'),
          auth: randomBytes(16).toString('base64url'),
        },
      },
    );
    const cdp = await context.newCDPSession(page);
    // Playwright enables focus emulation on every page; use real tab visibility.
    await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: false });
    await page.goto(base + '/relay-settings');
    const settingsDialog = page.getByRole('dialog', { name: 'Settings', exact: true });
    await settingsDialog.getByRole('tab', { name: /^Notifications/ }).click();
    await expect(page.getByRole('heading', { name: 'System notifications in this browser' })).toBeVisible();
    expect((await api('/relay/account/notifications/preferences')).scope).toBe('subscribed');
    await page
      .getByRole('button', { name: 'Enable notifications', exact: true })
      .click();
    await expect
      .poll(
        async () =>
          (await api('/relay/account/notifications')).subscriptionIds.length,
      )
      .toBe(1);
    await expect(
      page.getByText('Enabled in this browser', { exact: true }),
    ).toBeVisible();
    expect(
      (await api('/relay/account/notifications')).subscriptionIds,
    ).toHaveLength(1);
    await page
      .getByRole('button', { name: 'Disable notifications', exact: true })
      .click();
    await expect(
      page.getByText('Disabled in this browser', { exact: true }),
    ).toBeVisible();
    expect(
      (await api('/relay/account/notifications')).subscriptionIds,
    ).toHaveLength(0);

    const prefix = device.api;
    const id = device.thread;
    const url = device.url;
    const expectTab = async (shape: string) => {
      await expect(page).toHaveTitle('Signed thread 1');
      await expect
        .poll(
          () =>
            page.evaluate(() =>
              decodeURIComponent(
                document.querySelector<HTMLLinkElement>(
                  'link[type="image/svg+xml"]',
                )?.href ?? '',
              ),
            ),
          { timeout: 40000 },
        )
        .toContain(shape);
    };
    await page.goto(url);
    await expect(page.getByRole('textbox', { name: 'Prompt' })).toBeVisible();
    await expectTab('m8 16 5 5 11-11');
    await expect(page.getByRole('textbox', { name: 'Prompt', exact: true })).toHaveAttribute('contenteditable', /^(true|plaintext-only)$/);
    await page
      .getByRole('textbox', { name: 'Prompt' })
      .fill('Inspect this repository in depth');
    await page
      .getByRole('button', { name: 'Send Prompt', exact: true })
      .click();
    await expectTab('M25 16a9');
    await expect(page.getByRole('button', { name: 'Subscribe to this turn', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Subscribe to this turn', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Notify you when this turn ends', exact: true })).toHaveAttribute('aria-pressed', 'true');
    const foreground = await context.newPage();
    await foreground.goto(base + '/relay-account');
    await foreground.bringToFront();
    await expect
      .poll(() => page.evaluate(() => document.hasFocus()))
      .toBe(false);
    await expectTab('fill="#f85149"');
    const detail = await api(`${prefix}/threads/${id}`);
    const turn = detail.turns.at(-1).id;
    await expect.poll(async () => (await api('/relay/account/workbench')).notifications.length).toBe(1);
    expect((await api(`/relay/account/notifications/turns/${device.id}/${id}/${turn}`)).subscribed).toBe(false);
    // No browser subscription exists now, so no synthetic endpoint is contacted.
    // The authenticated device tunnel must still durably persist and ACK its event.
    const relayDb = new DatabaseSync(
      join(root, 'relay', 'relay-store.sqlite'),
      { readOnly: true },
    );
    const supervisorDb = new DatabaseSync(device.db, {
      readOnly: true,
    });
    try {
      await expect
        .poll(
          () =>
            relayDb
              .prepare('SELECT count(*) AS n FROM relay_push_events WHERE id=?')
              .get(`${device.id}:${turn}`)?.n,
        )
        .toBe(1);
      await expect
        .poll(
          () =>
            supervisorDb
              .prepare(
                "SELECT count(*) AS n FROM kv WHERE key GLOB 'relay:notice:*'",
              )
              .get()?.n,
        )
        .toBe(0);
    } finally {
      relayDb.close();
      supervisorDb.close();
    }
    await page.bringToFront();
    await expectTab('m8 16 5 5 11-11');
    await page.reload();
    await expectTab('m8 16 5 5 11-11');

    // Deliver through Chromium's actual Service Worker push dispatcher. This
    // verifies display/data with our built worker, not an external push vendor.
    let registrationId = '';
    cdp.on('ServiceWorker.workerRegistrationUpdated', ({ registrations }) => {
      registrationId =
        registrations.find(
          (r: { scopeURL: string }) => r.scopeURL === base + '/',
        )?.registrationId ?? registrationId;
    });
    await cdp.send('ServiceWorker.enable');
    await expect.poll(() => registrationId).not.toBe('');
    await cdp.send('ServiceWorker.deliverPushMessage', {
      origin: base,
      registrationId,
      data: JSON.stringify({
        url,
        body: 'A thread turn completed. Click to view.',
        tag: 'fixture-turn',
      }),
    });
    await expect
      .poll(() =>
        page.evaluate(async () =>
          (await (await navigator.serviceWorker.ready).getNotifications()).map(
            (n) => n.data.url,
          ),
        ),
      )
      .toContain(url);
  } finally {
    await fixture.close();
  }
});
