// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { ConfirmDialog } from '@pockymoe/thread-ui';
import { MobileHostBoundary } from './MobileHost';
import { playHaptic } from '../desktop/surface';
vi.mock('../desktop/surface', () => ({ useHostSurface: () => 'mobile', mobileHost: () => ({ onCommand: () => () => {}, window: { setTitle: vi.fn() }, appearance: { sync: vi.fn() } }), playHaptic: vi.fn() }));
vi.mock('../desktop/data', () => ({ DesktopIndexStore: vi.fn() }));
vi.mock('../desktop/integration', () => ({ focusedKeyFromPath: vi.fn(), handleHostCommand: vi.fn(), syncActivity: vi.fn() }));
vi.mock('../lib/themePacks', () => ({ useThemeRegistry: () => ({ applied: 'classic' }) }));
vi.mock('../lib/fontSize', () => ({ useFontSize: () => [16] }));
afterEach(() => { cleanup(); vi.mocked(playHaptic).mockClear(); });
it('warns on the shared destructive confirmation, including its icon target, and not Cancel or ordinary clicks', () => {
  render(<MemoryRouter><MobileHostBoundary effectiveTheme="light"><button className="ui-action-danger">Outside</button><ConfirmDialog open title="Delete fixture" description="Remove this test thread" confirmLabel="Confirm deletion" cancelLabel="Keep thread" onCancel={() => {}} onConfirm={() => {}} /></MobileHostBoundary></MemoryRouter>);
  fireEvent.click(screen.getByRole('button', { name: 'Outside' }));
  fireEvent.click(screen.getByRole('button', { name: 'Keep thread' })); expect(playHaptic).not.toHaveBeenCalled();
  const button = screen.getByRole('button', { name: 'Confirm deletion' });
  const icon = document.createElement('span'); button.append(icon); fireEvent.click(icon);
  expect(playHaptic).toHaveBeenCalledExactlyOnceWith('warning');
});
it('does not vibrate for the shared non-destructive confirmation', () => {
  render(<MemoryRouter><MobileHostBoundary effectiveTheme="dark"><ConfirmDialog open tone="primary" title="Fork fixture" description="Create a thread" confirmLabel="Create thread" onCancel={() => {}} onConfirm={() => {}} /></MobileHostBoundary></MemoryRouter>);
  fireEvent.click(screen.getByRole('button', { name: 'Create thread' })); expect(playHaptic).not.toHaveBeenCalled();
});
