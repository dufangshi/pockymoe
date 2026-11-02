import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { RelayDeviceDto } from '@pockymoe/shared';
import { UnprotectedDevicesButton } from './DevicesNavigation';
const state = vi.hoisted(() => ({ protection: 'protected' }));
vi.mock('../../lib/useDeviceProtection', () => ({ useDeviceProtection: () => state.protection }));
vi.mock('../../lib/relayTransport', () => ({ getTransportStatus: () => undefined }));
vi.mock('../../components/RemoteProtectionControls', () => ({ ProtectDevicesButton: () => <button>Protect all</button> }));
it.each([['protected', false], ['unknown', false], ['not-protected', true], ['certificate-missing', true], ['pending', true]] as const)('offers batch protection only for known unprotected devices (%s)', async (protection, visible) => {
  state.protection = protection;
  render(<UnprotectedDevicesButton devices={[{id:'one'} as RelayDeviceDto]} />);
  if (visible) expect(await screen.findByRole('button', { name: 'Protect all' })).toBeVisible();
  else expect(screen.queryByRole('button')).toBeNull();
});
