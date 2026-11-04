import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { RelayUserMenu } from './RelayUserMenu';

const actions = vi.hoisted(() => ({ settings: vi.fn(), tour: vi.fn(), attention: 0 }));
vi.mock('./sharedAccess/useAccountShareAttention', () => ({ useAccountShareAttention: () => ({ attention: new Map(Array.from({ length: actions.attention }, (_, i) => [String(i), 'pending'])) }) }));
vi.mock('./AppShellNavContext', () => ({ useAppShellNav: () => ({ openSettings: actions.settings }) }));
vi.mock('./onboarding/TourProvider', () => ({ useTour: () => ({ openHub: actions.tour }) }));
vi.mock('../lib/api', () => ({
  ApiError: class extends Error {},
  relayModeActive: () => true,
  fetchRelaySession: async () => ({ authenticated: true, user: { username: 'demo', email: 'demo@example.test' } }),
  relayLogout: vi.fn(),
}));

beforeEach(() => { vi.clearAllMocks(); actions.attention = 0; });
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

it('names share attention and opens the rail-specific settings dialog', async () => {
  actions.attention = 2;
  const settings = vi.fn();
  render(<MemoryRouter><RelayUserMenu rail onSettings={settings} /></MemoryRouter>);
  const trigger = await screen.findByRole('button', { name: /Relay account menu for demo.*Sharing: 2 to review/ });
  vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(8, 700, 40, 40));
  fireEvent.click(trigger);
  expect(screen.getByRole('menuitem', { name: /Device management.*2/ })).toBeVisible();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Settings' }));
  expect(settings).toHaveBeenCalledOnce(); expect(actions.settings).not.toHaveBeenCalled();
});

it('opens a phone top-bar avatar downward and right-aligned', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener() {}, removeEventListener() {} }));
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 390 });
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: 844 });
  render(<MemoryRouter><RelayUserMenu rail /></MemoryRouter>);
  const trigger = await screen.findByRole('button', { name: 'Relay account menu for demo' });
  vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(340, 20, 44, 44));
  fireEvent.click(trigger);
  const menu = screen.getByRole('menu');
  Object.defineProperty(menu, 'scrollHeight', { configurable: true, value: 320 });
  await act(async () => { window.dispatchEvent(new Event('resize')); await Promise.resolve(); });
  expect(menu.style.top).toBe('72px');
  expect(menu.style.left).toBe('126px');
});

it('opens a phone bottom avatar upward and stays inside the viewport', async () => {
  vi.stubGlobal('matchMedia', () => ({ matches: true, addEventListener() {}, removeEventListener() {} }));
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 390 });
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: 844 });
  render(<MemoryRouter><RelayUserMenu rail /></MemoryRouter>);
  const trigger = await screen.findByRole('button', { name: 'Relay account menu for demo' });
  vi.spyOn(trigger, 'getBoundingClientRect').mockReturnValue(new DOMRect(8, 780, 44, 44));
  fireEvent.click(trigger);
  const menu = screen.getByRole('menu');
  Object.defineProperty(menu, 'scrollHeight', { configurable: true, value: 320 });
  await act(async () => { window.dispatchEvent(new Event('resize')); await Promise.resolve(); });
  expect(menu.style.top).toBe('452px');
  expect(menu.style.left).toBe('8px');
  expect(Number.parseFloat(menu.style.top)).toBeGreaterThanOrEqual(8);
  expect(Number.parseFloat(menu.style.top) + 320).toBeLessThanOrEqual(836);
});
