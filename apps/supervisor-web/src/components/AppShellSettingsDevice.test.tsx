import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { PluginProvider, SettingsDialog } from '@pockymoe/thread-ui';
import { appSettingsSections, deviceSettingsSections } from './AppShellSettingsDialog';
import { fetchRelayPortal } from '../lib/api';
import { chooseSettingsDevice } from '../lib/settingsDevice';
import { encryptedBrowserFetch } from '../lib/relayTransport';

// Issue #112: the settings window read and saved the device remembered in
// localStorage (device-1) while the reader was on another device's page.
// Issue #115: in Relay mode a device's own tabs are its page's Settings tab
// (deviceSettingsSections); the window keeps Plugins, still for one named device.
vi.mock('../lib/relayTransport', async original => ({
  ...await original<typeof import('../lib/relayTransport')>(),
  encryptedBrowserFetch: vi.fn(),
}));
// The portal read also records owned devices in IndexedDB, which jsdom lacks.
vi.mock('../lib/api', async original => ({
  ...await original<typeof import('../lib/api')>(),
  fetchRelayPortal: vi.fn(),
}));

const DEVICE_TABS = ['harnesses', 'upstreams', 'resources', 'environments', 'device', 'workspace'];
const devices = [
  { id: 'device-0', name: 'MacBook Pro', connected: true },
  { id: 'device-1', name: 'Windows Studio', connected: false },
];
let portalDevices = devices;
/** Every request a device answered, as `METHOD device path`. */
let deviceRequests: string[] = [];

/** What a Supervisor answers; `device` names whose values these are. */
function answer(path: string, init: RequestInit, device: string) {
  if (path === 'api/config/workspace-settings') {
    return { workspaceRoot: `/root-of-${device}`, devHome: `/dev-of-${device}`, ...(init.method === 'PATCH' ? JSON.parse(String(init.body)) : {}) };
  }
  if (path === 'api/environments') return { connections: [], environments: [], budget: { maxRunning: 3, maxCpu: 8, maxMemoryBytes: 8 * 1024 ** 3, reservedCpu: 0, reservedMemoryBytes: 0 } };
  return path.endsWith('/harnesses') || path === 'api/plugins' || path.startsWith('api/environment-requests') ? [] : {};
}

beforeEach(() => {
  setLocale('en', false);
  localStorage.setItem('remote-codex-relay-mode', 'true');
  localStorage.setItem('remote-codex-relay-device-id', 'device-1');
  portalDevices = devices;
  deviceRequests = [];
  vi.mocked(fetchRelayPortal).mockReset().mockImplementation(async () => ({ user: { id: 'u', username: 'alex' }, devices: portalDevices, sharedWithMe: [], sharedByMe: [] }) as never);
  vi.mocked(encryptedBrowserFetch).mockReset().mockImplementation(async (url, init) => {
    const { pathname } = new URL(String(url), 'http://relay.test');
    const [, device = '', path = ''] = /\/relay\/devices\/([^/]+)\/(.*)$/.exec(pathname) ?? [];
    deviceRequests.push(`${init.method ?? 'GET'} ${device} ${path}`);
    return Response.json(answer(path, init, device));
  });
  vi.stubGlobal('fetch', vi.fn(async (url: string | URL, init: RequestInit = {}) => Response.json(answer(new URL(String(url), 'http://local.test').pathname.slice(1), init, 'local'))));
});
afterEach(() => {
  act(() => chooseSettingsDevice(null));
  localStorage.clear();
  vi.unstubAllGlobals();
});

function open(path: string, section: string, sections = appSettingsSections()) {
  return render(<MemoryRouter initialEntries={[path]}><PluginProvider adapter={{ fetchPlugins: () => [] }} builtinPlugins={[]}>
    <SettingsDialog open onOpenChange={() => {}} sections={sections} initialId={section} />
  </PluginProvider></MemoryRouter>);
}
/** A device page's Settings tab, for `device`, opened on `path`. */
const devicePage = (path: string, section: string, device = 'device-0') => open(path, section, deviceSettingsSections(device, null));
const devicesAsked = () => [...new Set(deviceRequests.map(entry => entry.split(' ')[1]))];

it('keeps the browser and the account in the window: a device\'s own tabs are on its page', () => {
  open('/desktop/devices/device-0', 'preferences');
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Preferences', 'Notifications', 'Plugins']);
});

it('reads and saves workspace defaults on the device whose page is open, not the remembered one', async () => {
  devicePage('/desktop/devices/device-0', 'workspace');
  const field = await screen.findByLabelText('Dev home');
  await waitFor(() => expect(field).toHaveValue('/dev-of-device-0'));
  expect(deviceRequests).toEqual(['GET device-0 api/config/workspace-settings']);
  fireEvent.change(field, { target: { value: '/elsewhere' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save workspace defaults' }));
  await waitFor(() => expect(deviceRequests).toContain('PATCH device-0 api/config/workspace-settings'));
  expect(devicesAsked()).toEqual(['device-0']);
  // The page names the device: no bar offers another.
  expect(screen.queryByRole('group', { name: 'Device these settings apply to' })).toBeNull();
});

it.each(DEVICE_TABS)('keeps the %s tab of a device page on that device', async section => {
  devicePage('/desktop/devices/device-0', section);
  await waitFor(() => expect(deviceRequests.length).toBeGreaterThan(0));
  expect(devicesAsked()).toEqual(['device-0']);
});

it('names the device and its connection state beside its plugins', async () => {
  open('/desktop/devices/device-0', 'plugins');
  const bar = await screen.findByRole('group', { name: 'Device these settings apply to' });
  expect(await within(bar).findByText('MacBook Pro')).toBeVisible();
  expect(within(bar).getByText('Online')).toBeVisible();
  // The page names the device: the window offers no other.
  expect(within(bar).queryByRole('combobox')).toBeNull();
  await waitFor(() => expect(devicesAsked()).toEqual(['device-0']));
});

it('keeps the plugins tab on the routed device under /devices/:id', async () => {
  open('/devices/device-0/workspaces', 'plugins');
  expect(await within(await screen.findByRole('group', { name: 'Device these settings apply to' })).findByText('MacBook Pro')).toBeVisible();
  // Plugins on a routed page are the page's own, already read by its provider.
  expect(devicesAsked().filter(device => device !== 'device-0')).toEqual([]);
});

// The map and the account pages show no single device.
it.each(['/', '/desktop/devices', '/relay-account'])('asks which device\'s plugins on %s instead of a dead end, then acts on the chosen one', async path => {
  open(path, 'plugins');
  const bar = await screen.findByRole('group', { name: 'Device these settings apply to' });
  const choice = await within(bar).findByRole('combobox');
  expect(choice).toHaveTextContent('Choose device');
  expect(screen.getByText('These settings belong to one device. Choose the device to manage.')).toBeVisible();
  expect(deviceRequests).toEqual([]);
  fireEvent.click(choice);
  fireEvent.click(await screen.findByRole('option', { name: 'MacBook Pro' }));
  await waitFor(() => expect(deviceRequests).toContain('GET device-0 api/plugins'));
  expect(devicesAsked()).toEqual(['device-0']);
  expect(within(bar).getByRole('combobox')).toHaveTextContent('MacBook Pro');
  expect(within(bar).getByText('Online')).toBeVisible();
});

it('uses the account\'s only device without asking, and names it', async () => {
  portalDevices = [devices[1]!];
  open('/desktop/devices', 'plugins');
  await waitFor(() => expect(deviceRequests).toEqual(['GET device-1 api/plugins']));
  const bar = screen.getByRole('group', { name: 'Device these settings apply to' });
  expect(within(bar).getByRole('combobox')).toHaveTextContent('Windows Studio');
  expect(within(bar).getByText('Offline')).toBeVisible();
});

it('keeps the settings of an owner that names the device on that device, without a second name', async () => {
  const { AppShellSettingsDialog } = await import('./AppShellSettingsDialog');
  render(<MemoryRouter initialEntries={['/desktop/devices/device-1']}><AppShellSettingsDialog embedded section="workspace" deviceId="device-0" /></MemoryRouter>);
  await waitFor(() => expect(deviceRequests).toEqual(['GET device-0 api/config/workspace-settings']));
  expect(screen.queryByRole('group', { name: 'Device these settings apply to' })).toBeNull();
});

it('shows no device bar for a local Supervisor', async () => {
  localStorage.removeItem('remote-codex-relay-mode');
  open('/workspaces', 'workspace');
  expect(await screen.findByLabelText('Dev home')).toBeVisible();
  expect(screen.queryByRole('group', { name: 'Device these settings apply to' })).toBeNull();
  expect(deviceRequests).toEqual([]);
});
