import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { expect, it } from 'vitest';
import { DevicesEntryRoute } from './DevicesEntryRoute';
function Destination() { const location = useLocation(); return <p>{location.pathname + location.search}</p>; }
it('replaces the old devices route on every surface and preserves all its query state', async () => {
  const query = '?tab=shared&dir=out&shareDevice=one&action=share';
  render(<MemoryRouter initialEntries={['/relay-devices' + query]}><Routes><Route path="/relay-devices" element={<DevicesEntryRoute />} /><Route path="/desktop/devices" element={<Destination />} /></Routes></MemoryRouter>);
  expect(await screen.findByText('/desktop/devices' + query)).toBeVisible();
});
