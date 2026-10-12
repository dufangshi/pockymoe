import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { notifyOwnerUnlocked } from '../lib/ownerUnlockRecovery';
import { exchange } from '../lib/relayTransportCrypto';
import { WorkspacesPage } from './WorkspacesPage';
import { encryptedBrowserFetch } from '../lib/relayTransport';

vi.mock('../lib/relayTransportCrypto', () => ({
  exchange: vi.fn(), setTransportReporter: vi.fn(),
  scopeFromPage: () => undefined,
  deviceRoute: (url: URL) => {
    const match = url.pathname.match(/^\/relay\/devices\/([^/]+)(\/api\/.*)$/);
    return match ? { deviceId: match[1], path: match[2] } : null;
  },
}));
vi.mock('../components/ProductHeader', () => ({ ProductHeader: () => null }));
vi.mock('../components/DeviceEncryptionStatus', () => ({ DeviceEncryptionStatus: () => null }));
vi.mock('../components/onboarding/TourProvider', () => ({ TourLauncherButton: () => null }));
let unlocked: boolean;
beforeEach(() => {
  vi.clearAllMocks();
  unlocked = false;
  window.history.replaceState({}, '', '/devices/a/workspaces?relay=1');
  Object.defineProperty(window, 'isSecureContext', { configurable: true, value: true });
  Object.defineProperty(navigator, 'serviceWorker', { configurable: true, value: {
    register: vi.fn().mockResolvedValue({}), ready: Promise.resolve({}),
    controller: { postMessage: vi.fn() }, addEventListener: vi.fn(),
  } });
  vi.mocked(exchange).mockImplementation(async request => {
    if (!unlocked) return { response: Response.json({ code: 'browser_registration_required' }, { status: 401 }) };
    if (request.url.endsWith('/workspaces')) return { response: Response.json([{
      id: 'w', label: 'Recovered workspace', absPath: '/tmp/work', createdAt: '2026-01-01', lastOpenedAt: null,
    }]) };
    if (request.url.endsWith('/transport/trust')) return { response: Response.json({
      deviceId: 'a', paired: true, strict: true, credentials: [], sessions: [], certificate: null,
    }) };
    return { response: Response.json({ workspaceRoot: '/tmp/work', environment: 'test', host: 'localhost', port: 0, appName: 'Pockymoe', appVersion: 'test' }) };
  });
});
function unlock(deviceIds = ['a']) {
  act(() => { if (deviceIds.includes('a')) unlocked = true; notifyOwnerUnlocked(deviceIds); });
}
describe('device views resume their original blocked loads', () => {
  it('loads the workspace list after matching unlock without Retry or a page reload, leaving completed data alone', async () => {
    render(<MemoryRouter><WorkspacesPage /></MemoryRouter>);
    await waitFor(() => expect(exchange).toHaveBeenCalledTimes(2));
    expect(screen.queryByText('Recovered workspace')).toBeNull();
    unlock(['b']);
    expect(exchange).toHaveBeenCalledTimes(2);
    unlock();
    expect(await screen.findByText('Recovered workspace')).toBeVisible();
    expect(screen.queryByRole('alert')).toBeNull();
    const completed = vi.mocked(exchange).mock.calls.length;
    unlock(['b']); unlock();
    expect(exchange).toHaveBeenCalledTimes(completed);
  });
  it('returns an owner error for mutations immediately and never replays them after unlocking', async () => {
    const response = await encryptedBrowserFetch('/relay/devices/a/api/workspaces', { method: 'POST', body: '{}' });
    expect(response.status).toBe(401);
    unlock();
    expect(exchange).toHaveBeenCalledTimes(1);
  });
});
