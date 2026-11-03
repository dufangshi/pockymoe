import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ActivityFamily } from './ActivitySection';
import type { ActivityItem } from './host';

afterEach(cleanup);
const root = { key: 'mac:root', id: 'root', deviceId: 'mac', title: 'app' };
const member = { key: 'mac:child', threadId: 'child', deviceId: 'mac', deviceName: 'mac', workspaceLabel: 'w', title: 'terminal', state: 'running', since: new Date().toISOString(), rootThreadKey: root.key } as unknown as ActivityItem;
const setup = () => {
  const onOpen = vi.fn(), menu = vi.fn((key: string) => <button>{`menu ${key}`}</button>);
  render(<ActivityFamily root={root} item={member} members={[member]} favorites={new Set()} onOpen={onOpen} menu={menu} />);
  return { onOpen };
};

it('opens the conversation from the row and keeps its agents folded', () => {
  const { onOpen } = setup();
  fireEvent.click(screen.getByRole('link', { name: /app/ }));
  expect(onOpen).toHaveBeenCalledWith('mac:root');
  expect(screen.queryByTitle('terminal')).toBeNull();
});
it('folds agent threads from the chevron without navigating', () => {
  const { onOpen } = setup();
  const toggle = screen.getByRole('button', { expanded: false });
  fireEvent.click(toggle);
  expect(screen.getByTitle('terminal')).toBeTruthy();
  expect(toggle.getAttribute('aria-expanded')).toBe('true');
  expect(onOpen).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTitle('terminal'));
  expect(onOpen).toHaveBeenCalledWith('mac:child');
});
it('leaves a modified click to the browser and gives the conversation its menu', () => {
  const { onOpen } = setup();
  fireEvent.click(screen.getByRole('link', { name: /app/ }), { metaKey: true });
  expect(onOpen).not.toHaveBeenCalled();
  expect(screen.getByText('menu mac:root')).toBeTruthy();
});
