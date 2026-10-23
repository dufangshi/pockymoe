import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import { HostCapabilitiesProvider, HostFeatureGate } from './hostCapabilities';
import { fetchRuntimeConfig, relayModeActive } from './api';

vi.mock('./api', () => ({ fetchRuntimeConfig: vi.fn(), relayModeActive: vi.fn(() => false) }));
afterEach(() => { cleanup(); vi.mocked(fetchRuntimeConfig).mockReset(); vi.mocked(relayModeActive).mockReturnValue(false); });

function Entries({ scope = 'local' }: { scope?: string }) {
  return <HostCapabilitiesProvider scope={scope}>
    <button>Local threads</button>
    <HostFeatureGate feature="relay"><button>Device management</button></HostFeatureGate>
    <HostFeatureGate feature="accountBinding"><button>Bind account</button></HostFeatureGate>
    <HostFeatureGate feature="devicePeer"><button>Other devices</button></HostFeatureGate>
    <HostFeatureGate feature="previews"><button>Port previews</button></HostFeatureGate>
  </HostCapabilitiesProvider>;
}

it('hides disabled entry points while keeping local threads available', async () => {
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({ hostCapabilities: {
    relay: false, accountBinding: false, devicePeer: false, previews: false,
  } } as RuntimeConfigDto);
  render(<Entries />);
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Port previews' })).toBeNull());
  for (const name of ['Device management', 'Bind account', 'Other devices']) {
    expect(screen.queryByRole('button', { name })).toBeNull();
  }
  expect(screen.getByRole('button', { name: 'Local threads' })).toBeVisible();
});

it('retains standalone and older Supervisor entry points', async () => {
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({} as RuntimeConfigDto);
  render(<Entries />);
  await waitFor(() => expect(fetchRuntimeConfig).toHaveBeenCalledOnce());
  expect(screen.getAllByRole('button')).toHaveLength(5);
});

it('does not ask an arbitrary relay device to describe relay pages', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  render(<Entries />);
  expect(screen.getAllByRole('button')).toHaveLength(5);
  expect(fetchRuntimeConfig).not.toHaveBeenCalled();
});

it('asks the device in the path and keeps its pages when it cannot answer', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  vi.mocked(fetchRuntimeConfig).mockRejectedValue(new Error('offline'));
  render(<Entries scope="device-1" />);
  expect(await screen.findByRole('button', { name: 'Local threads' })).toBeVisible();
  expect(fetchRuntimeConfig).toHaveBeenCalledWith('device-1');
  expect(screen.queryByRole('alert')).toBeNull();
});

it('still blocks a local host whose scope is unknown', async () => {
  vi.mocked(fetchRuntimeConfig).mockRejectedValue(new Error('offline'));
  render(<Entries />);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Local threads' })).toBeNull();
});

it('keeps the app mounted when the relay moves to another device', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  let answerB: (config: RuntimeConfigDto) => void = () => {};
  vi.mocked(fetchRuntimeConfig).mockImplementation(async (scope) => scope === 'device-b'
    ? new Promise<RuntimeConfigDto>(resolve => { answerB = resolve; })
    : ({ hostCapabilities: { relay: true, accountBinding: true, devicePeer: true, previews: true } } as RuntimeConfigDto));
  const view = render(<Entries scope="device-a" />);
  const local = await screen.findByRole('button', { name: 'Local threads' });
  view.rerender(<Entries scope="device-b" />);
  // Device B has not answered yet; the page stays (the same element, not a remount).
  expect(screen.getByRole('button', { name: 'Local threads' })).toBe(local);
  answerB({ hostCapabilities: { relay: true, accountBinding: true, devicePeer: true, previews: false } } as RuntimeConfigDto);
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Port previews' })).toBeNull());
  // Back on A: its answer shows at once.
  view.rerender(<Entries scope="device-a" />);
  expect(screen.getByRole('button', { name: 'Port previews' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Local threads' })).toBe(local);
});

it('still waits for a local host to state its scope before showing anything', async () => {
  vi.mocked(fetchRuntimeConfig).mockImplementation(() => new Promise(() => {}));
  render(<Entries />);
  await new Promise(resolve => setTimeout(resolve, 20));
  expect(screen.queryByRole('button')).toBeNull();
});
