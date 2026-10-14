import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { RuntimeConfigDto } from '@pockymoe/shared';
import { HostCapabilitiesProvider, HostFeatureGate } from './hostCapabilities';
import { fetchRuntimeConfig } from './api';

vi.mock('./api', () => ({ fetchRuntimeConfig: vi.fn() }));
afterEach(() => { cleanup(); vi.mocked(fetchRuntimeConfig).mockReset(); });

function Entries() {
  return <HostCapabilitiesProvider scope="local">
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
