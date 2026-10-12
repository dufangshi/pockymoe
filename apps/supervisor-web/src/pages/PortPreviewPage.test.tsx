import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, expect, it, vi } from 'vitest';
import { PortPreviewPage } from './PortPreviewPage';
import { openDevicePortPreview } from '../lib/portPreview';
vi.mock('../lib/portPreview', () => ({ openDevicePortPreview: vi.fn() }));
beforeEach(() => { vi.mocked(openDevicePortPreview).mockReset().mockRejectedValue(new Error('stop before redirect')); });
it.each([
  ['?lease=cli_lease&leaseExpiresAt=1760000000000&path=%2Fconsole%3Ftoken%3Dlaunch', { leaseId: 'cli_lease', expiresAt: 1760000000000 }, '/console?token=launch'],
  ['?lease=older_cli_lease', { leaseId: 'older_cli_lease' }, '/'],
  ['?lease=cli_lease&leaseExpiresAt=invalid', { leaseId: 'cli_lease' }, '/'],
  ['', undefined, '/'],
])('passes a provided CLI lease and its expiry without including it in the app path: %s', async (query, lease, path) => {
  render(<MemoryRouter initialEntries={[`/devices/device/ports/mapping${query}`]}><Routes><Route path="/devices/:deviceId/ports/:mappingId" element={<PortPreviewPage />} /></Routes></MemoryRouter>);
  expect(await screen.findByRole('alert')).toHaveTextContent('stop before redirect');
  expect(openDevicePortPreview).toHaveBeenCalledWith('device', 'mapping', path, expect.any(AbortSignal), lease);
});
