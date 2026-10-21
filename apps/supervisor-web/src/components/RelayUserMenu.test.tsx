import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { RelayUserMenu } from './RelayUserMenu';

const actions = vi.hoisted(() => ({ settings: vi.fn(), tour: vi.fn() }));
vi.mock('./AppShellNavContext', () => ({ useAppShellNav: () => ({ openSettings: actions.settings }) }));
vi.mock('./onboarding/TourProvider', () => ({ useTour: () => ({ openHub: actions.tour }) }));
vi.mock('../lib/api', () => ({
  ApiError: class extends Error {},
  relayModeActive: () => true,
  fetchRelaySession: async () => ({ authenticated: true, user: { username: 'demo', email: 'demo@example.test' } }),
  relayLogout: vi.fn(),
}));

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.unstubAllGlobals());

async function openMenu() {
  render(<MemoryRouter><RelayUserMenu /></MemoryRouter>);
  const trigger = await screen.findByRole('button', { name: 'Relay account menu for demo' });
  vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(12, 0, 44, 44));
  fireEvent.click(trigger);
  return trigger;
}

it('keeps a menu action mounted through a touch blur without a related target', async () => {
  await openMenu();
  const menu = screen.getByRole('menu');
  const account = screen.getByRole('menuitem', { name: 'Account settings' });
  await waitFor(() => expect(account).toHaveFocus());
  // Some touch focus changes provide no relatedTarget before the click arrives.
  fireEvent.blur(account, { relatedTarget: null });
  const settings = screen.getByRole('menuitem', { name: 'Settings' });
  expect(menu.parentElement).toBe(document.body);
  fireEvent.pointerDown(settings);
  expect(screen.getByRole('menu')).toBe(menu);
  fireEvent.click(settings);
  expect(actions.settings).toHaveBeenCalledOnce();
  expect(screen.queryByRole('menu')).toBeNull();
});

it('uses the shared menu keyboard and closes on an outside press', async () => {
  const trigger = await openMenu();
  fireEvent.keyDown(screen.getByRole('menu'), { key: 'End' });
  expect(screen.getByRole('menuitem', { name: 'Log out' })).toHaveFocus();
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' });
  expect(screen.getByRole('menuitem', { name: 'Account settings' })).toHaveFocus();
  fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
  expect(screen.queryByRole('menu')).toBeNull();
  expect(trigger).toHaveFocus();
  fireEvent.keyDown(trigger, { key: 'ArrowUp' });
  expect(screen.getByRole('menuitem', { name: 'Log out' })).toHaveFocus();
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole('menu')).toBeNull();
});

it('repositions when browser chrome changes only the visual viewport', async () => {
  const viewport = Object.assign(new EventTarget(), { width: 390, height: 700, offsetLeft: 0, offsetTop: 0 });
  vi.stubGlobal('visualViewport', viewport);
  await openMenu();
  const menu = screen.getByRole('menu');
  act(() => {
    viewport.height = 300;
    viewport.offsetLeft = 10;
    viewport.dispatchEvent(new Event('resize'));
  });
  expect(menu).toHaveStyle({ maxHeight: '284px', left: '18px' });
  act(() => {
    viewport.offsetTop = 60;
    viewport.dispatchEvent(new Event('scroll'));
  });
  expect(screen.queryByRole('menu')).toBeNull();
});
