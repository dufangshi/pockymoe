import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { request, relayModeActive } from '../lib/api';
import { NotificationSettings } from './NotificationSettings';
import { TurnNotification } from './TurnNotification';
import type { NotificationSettings as Settings } from '../lib/notificationSettings';

vi.mock('../lib/api', () => ({ request: vi.fn(), relayModeActive: vi.fn(() => true) }));
vi.mock('./RelayNotifications', () => ({ RelayNotifications: () => null }));
let settings: Settings;
let subscribed = false;
let failSave = false;
let failLoad = false;
const subscriptions = new Map<string, boolean>();
beforeEach(() => {
  setLocale('zh-CN', false);
  settings = { scope: 'subscribed', devices: [], workspaces: [] };
  subscribed = false; failSave = false; failLoad = false; subscriptions.clear();
  vi.mocked(relayModeActive).mockReturnValue(true);
  vi.mocked(request).mockReset().mockImplementation(async (path, init) => {
    path = String(path);
    if (path.endsWith('/preferences')) {
      if (init?.method === 'PUT') {
        if (failSave) throw new Error('offline');
        settings = JSON.parse(String(init.body));
      }
      return structuredClone(settings);
    }
    if (path.includes('/turns/')) {
      if (failLoad && init?.method !== 'PUT') throw new Error('offline');
      if (init?.method === 'PUT') {
        if (failSave) throw new Error('offline');
        subscribed = JSON.parse(String(init.body)).subscribed;
        subscriptions.set(path,subscribed);
      }
      return { subscribed: subscriptions.get(path) ?? false };
    }
    if (path === 'relay/portal') return { devices: [{ id: 'one', name: '设备一', connected: true }, { id: 'two', name: '设备二', connected: true }] };
    if (path === 'relay/devices/two/api/workspaces') return [{ id: 'same', label: '工作区', absPath: '/test/work' }];
    throw new Error(`Unexpected ${path}`);
  });
});

it('saves each scope immediately and adds a workspace through the existing device-first picker, with undo', async () => {
  render(<NotificationSettings />);
  await waitFor(() => expect(screen.getByRole('radio', { name: /仅订阅的轮次/ })).toBeEnabled());
  expect(screen.getByRole('radio', { name: /仅订阅的轮次/ })).toBeChecked();
  fireEvent.click(screen.getByRole('radio', { name: /指定设备和工作区/ }));
  await screen.findByText('已保存');
  fireEvent.click(screen.getByRole('button', { name: '添加工作区' }));
  const dialog = screen.getByRole('dialog');
  expect(within(dialog).queryByText('工作区')).toBeNull();
  fireEvent.click(await within(dialog).findByRole('button', { name: /设备二/ }));
  fireEvent.click(await within(dialog).findByRole('button', { name: /工作区.*test/ }));
  await waitFor(() => expect(settings.workspaces).toEqual([{ deviceId: 'two', workspaceId: 'same', name: '工作区', deviceName: '设备二' }]));
  expect(screen.queryByRole('dialog')).toBeNull();
  await waitFor(() => expect(screen.getByRole('button', { name: '添加工作区' })).toHaveFocus());
  fireEvent.click(screen.getByRole('button', { name: '移除 工作区' }));
  fireEvent.click(await screen.findByRole('button', { name: '撤销' }));
  await waitFor(() => expect(settings.workspaces).toHaveLength(1));
  fireEvent.click(screen.getByRole('button', { name: '添加设备' }));
  fireEvent.click(await screen.findByRole('button', { name: /设备一/ }));
  await waitFor(() => expect(settings.devices).toEqual([{ deviceId: 'one', name: '设备一' }]));
  expect(vi.mocked(request).mock.calls.some(([p]) => String(p).includes('/api/threads'))).toBe(false);
});

it('rolls a failed preference save back and retries the chosen scope', async () => {
  render(<NotificationSettings />);
  await waitFor(() => expect(screen.getByRole('radio', { name: /所有轮次/ })).toBeEnabled());
  failSave = true;
  fireEvent.click(screen.getByRole('radio', { name: /所有轮次/ }));
  await screen.findByRole('alert');
  expect(screen.getByRole('radio', { name: /仅订阅/ })).toBeChecked();
  failSave = false;
  // The reason and its Retry open from the small mark.
  fireEvent.click(screen.getByRole('button', { name: '查看失败原因' }));
  fireEvent.click(screen.getByRole('button', { name: '重试' }));
  await waitFor(() => expect(settings.scope).toBe('all'));
});

it('keeps the shown choices without a mark when a refresh on focus fails', async () => {
  render(<NotificationSettings />);
  await waitFor(() => expect(screen.getByRole('radio', { name: /仅订阅的轮次/ })).toBeEnabled());
  const calls = vi.mocked(request).mock.calls.length;
  vi.mocked(request).mockRejectedValue(new Error('offline'));
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(vi.mocked(request).mock.calls.length).toBeGreaterThan(calls));
  await waitFor(() => expect(screen.getByRole('status')).toBeEmptyDOMElement());
  expect(screen.getByRole('radio', { name: /仅订阅的轮次/ })).toBeChecked();
  expect(screen.queryByRole('alert')).toBeNull();
});

it('restores a subscribed turn after remount, cancels it, isolates the next turn and hides the bell for all scope', async () => {
  const props = { deviceId: 'one', threadId: 'thread', turnId: 'turn' };
  const mounted = render(<TurnNotification {...props} />);
  const button = await screen.findByRole('button', { name: '订阅这一轮的通知' });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await waitFor(() => expect(screen.getByRole('button', { name: '结束时通知你' })).toBeEnabled());
  expect(screen.getByRole('button', { name: '结束时通知你' })).toHaveAttribute('aria-pressed', 'true');
  mounted.unmount();
  const restored = render(<TurnNotification {...props} />);
  await waitFor(() => expect(screen.getByRole('button', { name: '结束时通知你' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: '结束时通知你' }));
  await waitFor(() => expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toBeEnabled());
  expect(subscribed).toBe(false);
  restored.unmount();
  const next = render(<TurnNotification {...props} turnId="next" />);
  await waitFor(() => expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toBeEnabled());
  expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toHaveAttribute('aria-pressed', 'false');
  settings.scope = 'all';
  act(() => window.dispatchEvent(new Event('focus')));
  await waitFor(() => expect(screen.queryByRole('button', { name: /通知/ })).toBeNull());
  next.unmount();
});

it('keeps a failed subscription read quiet without silently subscribing, and rolls back failed writes', async () => {
  failLoad = true;
  render(<TurnNotification deviceId="two" threadId="thread" turnId="turn" />);
  // A background read that fails reports nothing and keeps the bell as it was.
  await waitFor(() => expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toBeEnabled());
  expect(vi.mocked(request).mock.calls.some(([path]) => String(path).includes('/turns/'))).toBe(true);
  expect(screen.queryByRole('alert')).toBeNull();
  expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toHaveAttribute('aria-pressed','false');
  failLoad = false;
  expect(vi.mocked(request).mock.calls.filter(([,init]) => init?.method === 'PUT')).toHaveLength(0);
  failSave = true;
  fireEvent.click(screen.getByRole('button', { name: '订阅这一轮的通知' }));
  await screen.findByRole('alert');
  expect(screen.getByRole('button', { name: '订阅这一轮的通知' })).toHaveAttribute('aria-pressed','false');
  failSave = false;
  // The reason and its Retry open from the small mark.
  fireEvent.click(screen.getByRole('button', { name: '查看失败原因' }));
  fireEvent.click(screen.getByRole('button', { name: '重试' }));
  await waitFor(() => expect(screen.getByRole('button', { name: '结束时通知你' })).toBeEnabled());
  expect(subscriptions.get('relay/account/notifications/turns/two/thread/turn')).toBe(true);
});
