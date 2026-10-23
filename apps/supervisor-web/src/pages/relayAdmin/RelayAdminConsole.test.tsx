// The console against responses captured from a real relay (__fixtures__):
// navigation, figures, paged tables, the detail window and admin actions.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import type { RelaySessionDto } from '@pockymoe/shared';
import overview from './__fixtures__/overview.json';
import users from './__fixtures__/users.json';
import userDetail from './__fixtures__/user-detail.json';
import registrations from './__fixtures__/registrations.json';
import devices from './__fixtures__/devices.json';
import deviceDetail from './__fixtures__/device-detail.json';
import shares from './__fixtures__/shares.json';
import publicLinks from './__fixtures__/public-links.json';
import previews from './__fixtures__/previews.json';
import security from './__fixtures__/security.json';
import securityEvents from './__fixtures__/security-events.json';
import sessions from './__fixtures__/sessions.json';
import system from './__fixtures__/system.json';
import search from './__fixtures__/search.json';
import overviewRecorded from './__fixtures__/overview-recorded.json';
import devicesRecorded from './__fixtures__/devices-recorded.json';
import deviceDetailRecorded from './__fixtures__/device-detail-recorded.json';
import securityRecorded from './__fixtures__/security-recorded.json';
import systemRecorded from './__fixtures__/system-recorded.json';
import actions from './__fixtures__/actions.json';
import retention from './__fixtures__/retention.json';
import performance from './__fixtures__/performance.json';

// Responses of a relay that records statistics (or of one that does not).
let recorded = false;

const calls: Array<{ path: string; method: string }> = [];

const api = vi.hoisted(() => ({
  setRelayUserEnabled: vi.fn(async () => ({})),
  approveRelayRegistration: vi.fn(async () => ({})),
  rejectRelayRegistration: vi.fn(async () => ({})),
  deleteRelayAdminUser: vi.fn(async () => ({})),
  resetRelayAdminUserPassword: vi.fn(async () => ({})),
}));

function respond(path: string, method: string) {
  const route = path.replace(/^relay\/admin\/?/, '').split('?')[0] ?? '';
  // Writes answer as the relay does: retention returns the new settings.
  if (method !== 'GET') return route === 'settings/retention' ? { ...retention, metricsDays: 30 } : { id: route.split('/').pop(), ok: true };
  const routes: Record<string, unknown> = {
    overview: recorded ? overviewRecorded : overview,
    users,
    registrations,
    devices: recorded ? devicesRecorded : devices,
    shares,
    'public-links': publicLinks,
    previews,
    security: recorded ? securityRecorded : security,
    'security/events': securityEvents,
    sessions,
    system: recorded ? systemRecorded : system,
    search,
    performance,
    ...(recorded ? { actions, 'settings/retention': retention } : {}),
    'settings/registration': { registrationEnabled: true, settings: { enabled: true, approvalRequired: true, registrationPasswordConfigured: false, googleAuthEnabled: false, githubAuthEnabled: false, emailVerificationEnabled: false, googleAuthAvailable: false, githubAuthAvailable: false, emailVerificationAvailable: false } },
  };
  if (route in routes) return routes[route];
  if (route.startsWith('users/')) return userDetail;
  if (route.startsWith('devices/')) return recorded ? deviceDetailRecorded : deviceDetail;
  throw Object.assign(new Error('not found'), { statusCode: 404 });
}

vi.mock('../../lib/api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../lib/api')>();
  return {
    ...actual,
    ...api,
    request: vi.fn(async (input: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET';
      calls.push({ path: String(input), method });
      const value = respond(String(input), method);
      // Keep the copy the page receives separate from the fixture.
      return structuredClone(value);
    }),
  };
});

vi.mock('../../components/RelaySecurity', () => ({
  RelaySecurityPanel: () => <div>security panel</div>,
}));

const { RelayAdminConsole } = await import('./RelayAdminConsole');

const session = { authenticated: true, registrationEnabled: true, user: { id: 'admin', username: 'opsadmin', email: 'ops@relay.example', role: 'admin', enabled: true, createdAt: '2026-01-01T00:00:00Z' } } as RelaySessionDto;

function renderConsole(path = '/relay-admin') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <RelayAdminConsole onLogout={() => undefined} onUnauthorized={() => undefined} session={session} />
    </MemoryRouter>,
  );
}

beforeAll(() => {
  setLocale('en', false);
  // jsdom lays nothing out; charts measure their container.
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 640 });
});
afterEach(() => {
  recorded = false;
  calls.length = 0;
  vi.clearAllMocks();
});

describe('relay admin console', () => {
  it('opens on the overview with what needs attention and the navigation counts', async () => {
    renderConsole();
    expect(await screen.findByText(/^registrations waiting for approval · oldest sent/)).toBeInTheDocument();
    expect(screen.getByText('accounts without an account key')).toBeInTheDocument();
    expect(screen.getByText('public links still stored as legacy plaintext')).toBeInTheDocument();
    const nav = screen.getByRole('navigation', { name: 'Admin sections' });
    expect(within(nav).getByRole('button', { name: /Users\s*3/ })).toBeInTheDocument();
    expect(within(nav).getByRole('button', { name: /Devices\s*17/ })).toBeInTheDocument();
    expect(within(nav).getByRole('button', { name: /Overview/ })).toHaveAttribute('aria-current', 'page');
    // The relay does not record history yet: no empty chart pretends zeros.
    expect(screen.queryByText('Online devices', { selector: 'h3' })).toBeNull();
    // A range applies to the overview's figures.
    fireEvent.click(screen.getByRole('button', { name: '7 days' }));
    await waitFor(() => expect(calls.some((call) => call.path === 'relay/admin/overview?range=7d')).toBe(true));
  });

  it('lists users with filters and pages, and opens a user in the detail window', async () => {
    renderConsole('/relay-admin?tab=users');
    const table = await screen.findByRole('table');
    expect(within(table).getAllByRole('row')).toHaveLength(users.items.length + 1);
    expect(screen.getByText(`1–${users.items.length} of ${users.total}`)).toBeInTheDocument();
    // The last sign-in and its browser come from the relay's own record.
    const linyue = users.items.find((user) => user.username === 'linyue')!;
    expect(within(table).getAllByText(linyue.lastSignInBrowser!).length).toBeGreaterThan(0);
    expect(within(table).getByRole('columnheader', { name: /Registered/ })).toBeInTheDocument();
    // A column filter: the account key menu lists both states with counts.
    fireEvent.click(within(screen.getByRole('group', { name: 'Filter users' })).getByRole('button', { name: /^Account key/ }));
    const keyMenu = await screen.findByRole('menu', { name: 'Account key' });
    fireEvent.click(within(keyMenu).getByRole('menuitemradio', { name: new RegExp(`Not set\\s*${users.counts.noAccountKey}`) }));
    await waitFor(() => expect(calls.some((call) => call.path.startsWith('relay/admin/users?') && call.path.includes('key=missing'))).toBe(true));
    expect(screen.getByRole('button', { name: 'Clear Account key' })).toBeInTheDocument();
    // A legacy link (`filter=noAccountKey`) lands on the same filter.
    cleanupAndRender('/relay-admin?tab=users&filter=noAccountKey');
    await screen.findByRole('table');
    await waitFor(() => expect(calls.filter((call) => call.path.includes('key=missing')).length).toBeGreaterThan(1));
    fireEvent.click(screen.getAllByRole('button', { name: 'Open linyue' })[0]!);
    const dialog = await screen.findByRole('dialog', { name: 'linyue' });
    expect(within(dialog).getByText('Sessions')).toBeInTheDocument();
    expect(within(dialog).getAllByRole('button', { name: 'End' }).length).toBe(userDetail.sessions.length);
    await act(async () => {
      fireEvent.keyDown(window, { key: 'Escape' });
    });
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'linyue' })).toBeNull());
  });

  it('sorts by a header (asc, desc, none) on the relay and keeps the sort in the URL and per table', async () => {
    localStorage.clear();
    renderConsole('/relay-admin?tab=users');
    const table = await screen.findByRole('table');
    const header = within(table).getByRole('columnheader', { name: /Last sign-in/ });
    expect(header).toHaveAttribute('aria-sort', 'none');
    fireEvent.click(within(header).getByRole('button'));
    await waitFor(() => expect(calls.some((call) => call.path.includes('sort=lastSignIn') && call.path.includes('dir=desc'))).toBe(true));
    expect(within(table).getByRole('columnheader', { name: /Last sign-in/ })).toHaveAttribute('aria-sort', 'descending');
    fireEvent.click(within(within(table).getByRole('columnheader', { name: /Last sign-in/ })).getByRole('button'));
    await waitFor(() => expect(within(table).getByRole('columnheader', { name: /Last sign-in/ })).toHaveAttribute('aria-sort', 'ascending'));
    fireEvent.click(within(within(table).getByRole('columnheader', { name: /Last sign-in/ })).getByRole('button'));
    await waitFor(() => expect(within(table).getByRole('columnheader', { name: /Last sign-in/ })).toHaveAttribute('aria-sort', 'none'));
    expect(JSON.parse(localStorage.getItem('pockymoe-admin-table:users') ?? '{}').sort).toBeNull();
  });

  it('shows and hides columns from the column chooser and remembers it', async () => {
    localStorage.clear();
    renderConsole('/relay-admin?tab=devices');
    const table = await screen.findByRole('table');
    expect(within(table).queryByRole('columnheader', { name: /Owner/ })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Columns' }));
    const menu = await screen.findByRole('menu', { name: 'Columns of Devices' });
    fireEvent.click(within(menu).getByRole('menuitemcheckbox', { name: 'Owner' }));
    fireEvent.click(within(menu).getByRole('menuitemcheckbox', { name: 'Protection' }));
    expect(within(table).getByRole('columnheader', { name: /Owner/ })).toBeInTheDocument();
    expect(within(table).queryByRole('columnheader', { name: /Protection/ })).toBeNull();
    expect(JSON.parse(localStorage.getItem('pockymoe-admin-table:devices') ?? '{}')).toMatchObject({ shown: ['owner'], hidden: ['protection'] });
    // A column can be widened from the keyboard; the width is kept.
    const handle = within(table).getByRole('separator', { name: 'Resize Connection' });
    fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(JSON.parse(localStorage.getItem('pockymoe-admin-table:devices') ?? '{}').widths.state).toBeGreaterThan(0);
    // The version filter offers what devices reported.
    fireEvent.click(within(screen.getByRole('group', { name: 'Filter devices' })).getByRole('button', { name: /^Version/ }));
    const versions = await screen.findByRole('menu', { name: 'Version' });
    fireEvent.click(within(versions).getByRole('menuitemradio', { name: new RegExp(`^${devices.versions![0]!.version.replace(/\./g, '\\.')}`) }));
    await waitFor(() => expect(calls.some((call) => call.path.includes(`version=v%3A${devices.versions![0]!.version}`))).toBe(true));
  });

  it('folds the sidebar into an icon rail and remembers it', async () => {
    localStorage.clear();
    const { container } = renderConsole('/relay-admin?tab=users');
    const toggle = await screen.findByRole('button', { name: 'Collapse sidebar' });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(toggle);
    expect(container.querySelector('.admin-console')).toHaveClass('is-collapsed');
    expect(localStorage.getItem('pockymoe-relay-admin-sidebar')).toBe('collapsed');
    // The names stay the buttons' accessible names.
    const nav = screen.getByRole('navigation', { name: 'Admin sections' });
    expect(within(nav).getByRole('button', { name: /^Users/ })).toHaveAttribute('data-tip', 'Users');
    cleanupAndRender('/relay-admin?tab=users');
    expect(await screen.findByRole('button', { name: 'Expand sidebar' })).toHaveAttribute('aria-expanded', 'false');
  });

  it('shows forwarding in depth: errors by kind, interpolated percentiles and the devices behind them', async () => {
    renderConsole('/relay-admin?tab=performance');
    expect(await screen.findByRole('heading', { name: 'Errors by kind' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Latency percentiles' })).toBeInTheDocument();
    expect(screen.getByText(/a percentile is interpolated inside the bucket/, { ignore: 'script' })).toBeInTheDocument();
    expect(screen.getByText(/How latency is measured/)).toBeInTheDocument();
    const traffic = screen.getAllByRole('region', { name: 'Traffic per device' })[0]!;
    const busiest = performance.deviceTraffic.devices[0]!;
    fireEvent.click(within(traffic).getAllByRole('button', { name: `Open ${busiest.name}` })[0]!);
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    await waitFor(() => expect(calls.some((call) => call.path === `relay/admin/devices/${busiest.deviceId}`)).toBe(true));
    // Every chart has a table view.
    const errors = () => screen.getAllByRole('region', { name: 'Errors by kind' })[0]!;
    fireEvent.click(within(errors()).getByRole('button', { name: 'Show as table' }));
    expect(within(errors()).getByRole('columnheader', { name: 'Timeout' })).toBeInTheDocument();
    // A custom window asks the relay for it.
    cleanupAndRender('/relay-admin?tab=performance&range=custom&from=2026-10-01&to=2026-10-03');
    await screen.findByRole('heading', { name: 'Errors by kind' });
    expect(calls.some((call) => call.path.startsWith('relay/admin/performance?from='))).toBe(true);
  });

  it('asks before disabling a user and then calls the relay', async () => {
    renderConsole('/relay-admin?tab=users');
    await screen.findByRole('table');
    fireEvent.click(screen.getByRole('button', { name: 'Actions for linyue' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Disable' }));
    const confirm = await screen.findByRole('dialog', { name: 'Disable linyue?' });
    expect(api.setRelayUserEnabled).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole('button', { name: 'Disable' }));
    await waitFor(() => expect(api.setRelayUserEnabled).toHaveBeenCalledWith(expect.any(String), false));
  });

  it('shows shares of every scope without thread titles and revokes one as admin', async () => {
    renderConsole('/relay-admin?tab=shares');
    const table = await screen.findByRole('table');
    expect(within(table).getAllByText('Whole device').length).toBeGreaterThan(0);
    expect(within(table).getAllByText('Workspace').length).toBeGreaterThan(0);
    expect(document.body.textContent).not.toMatch(/Secret thread title/);
    const first = shares.items[0]!;
    fireEvent.click(screen.getByRole('button', { name: `Actions for the share from ${first.ownerUsername} to ${first.targetUsername}` }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Revoke' }));
    fireEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Revoke' }));
    await waitFor(() => expect(calls).toContainEqual({ path: `relay/admin/shares/${first.kind}/${first.id}`, method: 'DELETE' }));
  });

  it('opens a device and offers to disconnect its tunnel', async () => {
    renderConsole(`/relay-admin?tab=devices&id=${deviceDetail.device.id}`);
    const dialog = await screen.findByRole('dialog', { name: deviceDetail.device.name });
    expect(within(dialog).getByText('Device ID')).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Copy device ID' })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Disconnect tunnel' }));
    fireEvent.click(within(await screen.findByRole('dialog', { name: `Disconnect ${deviceDetail.device.name}?` })).getByRole('button', { name: 'Disconnect tunnel' }));
    await waitFor(() => expect(calls).toContainEqual({ path: `relay/admin/devices/${deviceDetail.device.id}/disconnect`, method: 'POST' }));
  });

  it('reads the security audit and the system checks', async () => {
    renderConsole('/relay-admin?tab=security');
    expect(await screen.findByText('Event details')).toBeInTheDocument();
    expect(screen.getAllByText('Signed in with a second factor').length).toBeGreaterThan(0);
    renderConsole('/relay-admin?tab=system');
    expect(await screen.findByText('Database integrity')).toBeInTheDocument();
    expect(screen.getByText('Legacy plaintext public links')).toBeInTheDocument();
  });

  it('draws recorded statistics: online devices, tunnels, versions, device history, admin actions', async () => {
    recorded = true;
    renderConsole();
    expect(await screen.findByRole('img', { name: /^Online devices: / })).toBeInTheDocument();
    expect(screen.getByText('devices reconnected 5 or more times in the last hour')).toBeInTheDocument();
    expect(screen.getByText('Tunnels and forwarding')).toBeInTheDocument();
    expect(screen.getByRole('img', { name: /^Device versions: 0\.13\.38/ })).toBeInTheDocument();
    const actionsCard = screen.getByRole('region', { name: 'Admin actions' });
    expect(within(actionsCard).getByText(/^Approved a registration · /)).toBeInTheDocument();
    cleanupAndRender(`/relay-admin?tab=devices&id=${deviceDetailRecorded.device.id}`);
    const dialog = await screen.findByRole('dialog', { name: deviceDetailRecorded.device.name });
    expect(within(dialog).getByRole('img', { name: /^Tunnel connected .* of the last 24 hours/ })).toBeInTheDocument();
    expect(within(dialog).getByText('Forwarded requests · 24 hours')).toBeInTheDocument();
    cleanupAndRender('/relay-admin?tab=security&sub=actions');
    expect(await screen.findByRole('tab', { name: 'Admin actions' })).toHaveAttribute('aria-selected', 'true');
    expect(await screen.findByText('Changed registration settings')).toBeInTheDocument();
    expect(screen.getByText('approvalRequired: false → true')).toBeInTheDocument();
    cleanupAndRender('/relay-admin?tab=settings');
    expect(await screen.findByRole('group', { name: 'Metrics history' })).toBeInTheDocument();
    fireEvent.click(within(screen.getByRole('group', { name: 'Metrics history' })).getByRole('button', { name: '30 days' }));
    await waitFor(() => expect(calls).toContainEqual({ path: 'relay/admin/settings/retention', method: 'PATCH' }));
  });
});

function cleanupAndRender(path: string) {
  cleanup();
  renderConsole(path);
}
