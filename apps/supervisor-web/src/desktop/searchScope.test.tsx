import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
const search = vi.hoisted(() => vi.fn(async (_q: string, _w: string | undefined, _offset: number, _signal: AbortSignal, deviceId: string) => ({ matches: [{ threadId: `${deviceId}-thread`, threadTitle: deviceId, workspaceId: 'w', workspaceLabel: 'Project', workspacePath: '/project', turnId: 'turn', itemId: 'item', role: 'user', text: 'needle' }], hasMore: false })));
vi.mock('../lib/api', () => ({ ApiError: class extends Error {}, searchConversations: search, fetchThreadConversationPage: vi.fn(), fetchThreadTurnDetail: vi.fn(), searchThreadMessages: vi.fn() }));
import { ConversationSearch } from '../components/ConversationSearch';
it('does not reuse another explicit device search or its cached results', async () => {
  setLocale('en', false);
  const props = { threadId: 'current', workspaceId: 'w', allowGlobal: true, initialScope: 'device' as const, turns: [], open: true, onOpen: vi.fn(), onClose: vi.fn(), onSelect: vi.fn(), onNavigate: vi.fn() };
  const view = render(<ConversationSearch {...props} deviceId="device-a" deviceLabel="A" />);
  fireEvent.change(screen.getByRole('combobox'), { target: { value: 'needle' } });
  await waitFor(() => expect(search).toHaveBeenCalledWith('needle', undefined, 0, expect.any(AbortSignal), 'device-a'));
  view.rerender(<ConversationSearch {...props} deviceId="device-b" deviceLabel="B" />);
  await waitFor(() => expect(search).toHaveBeenLastCalledWith('needle', undefined, 0, expect.any(AbortSignal), 'device-b'));
  const result = await screen.findByRole('option', { name: /device-b/ }); fireEvent.click(result);
  expect(props.onNavigate).toHaveBeenCalledWith(expect.objectContaining({ threadId: 'device-b-thread' }), 'needle');
  view.unmount();
});
