import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import { DevicesEntryRoute } from './DevicesEntryRoute';
const target = vi.hoisted(() => ({ path: '/desktop/devices' }));
vi.mock('../lib/devicesRoute', () => ({ devicesHref: () => target.path }));
vi.mock('./RelayDevicesPage', () => ({ RelayDevicesPage: () => <p>Phone devices</p> }));
function Destination() { const location = useLocation(); return <p>{location.pathname + location.search}</p>; }
it('replaces the legacy PC route and preserves all old query state', async () => {
  target.path = '/desktop/devices';
  const query = '?tab=shared&dir=out&shareDevice=one&action=share';
  render(<MemoryRouter initialEntries={['/relay-devices'+query]}><Routes><Route path="/relay-devices" element={<DevicesEntryRoute />} /><Route path="/desktop/devices" element={<Destination />} /></Routes></MemoryRouter>);
  expect(await screen.findByText('/desktop/devices'+query)).toBeVisible(); expect(screen.queryByText('Phone devices')).toBeNull();
});
it('keeps the existing page for the phone/mobile-host destination', () => {
  target.path = '/relay-devices';
  render(<MemoryRouter initialEntries={['/relay-devices?tab=shared&dir=out']}><DevicesEntryRoute /></MemoryRouter>);
  expect(screen.getByText('Phone devices')).toBeVisible();
});
