import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RuntimeConfigDto, ThreadDetailDto } from '@pockymoe/shared';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { RecentThreadMenu } from './RecentThreadMenu';
import { HostCapabilitiesProvider } from '../lib/hostCapabilities';
import { ApiError, fetchRuntimeConfig, request } from '../lib/api';
vi.mock('../lib/api', async original => ({ ...(await original<typeof import('../lib/api')>()), fetchRuntimeConfig:vi.fn(), request:vi.fn() }));
const thread = { key:'local:reply-1',title:'reply 1',href:'/threads/reply-1',favorite:false,subtitle:'Project',status:'idle' };
const detail = { thread:{id:'reply-1',title:'reply 1',workspaceId:'project'} } as ThreadDetailDto;
beforeEach(() => {
  setLocale('en',false);
  vi.mocked(fetchRuntimeConfig).mockResolvedValue({hostScope:{kind:'treerAgent',hubId:'hub',boundThreadId:'root'}} as RuntimeConfigDto);
  vi.mocked(request).mockResolvedValue(detail);
});
afterEach(() => { cleanup();vi.clearAllMocks(); });
function mount() {
  const renamed=vi.fn().mockResolvedValue(undefined), removed=vi.fn().mockResolvedValue(undefined), navigate=vi.fn(), favorite=vi.fn().mockResolvedValue(undefined);
  render(<HostCapabilitiesProvider scope="local"><RecentThreadMenu thread={thread} currentKey={thread.key} onFavorite={favorite} onRenamed={renamed} onRemoved={removed} onNavigate={navigate} /></HostCapabilitiesProvider>);
  return {renamed,removed,navigate,favorite};
}
it('offers scoped favorite, rename and confirmed deletion, with successful requests', async () => {
  const {renamed,removed,navigate}=mount();
  fireEvent.click(await screen.findByRole('button',{name:'Actions for reply 1'}));
  fireEvent.click(await screen.findByRole('button',{name:'Rename thread'}));
  fireEvent.change(screen.getByRole('textbox',{name:'Thread title'}),{target:{value:'renamed'}});
  fireEvent.click(screen.getByRole('button',{name:'Save'}));
  await waitFor(() => expect(renamed).toHaveBeenCalledWith('local:reply-1','renamed'));
  expect(request).toHaveBeenCalledWith('api/threads/reply-1',expect.objectContaining({method:'PATCH',body:'{"title":"renamed"}'}));
  fireEvent.click(screen.getByRole('button',{name:'Actions for reply 1'}));
  fireEvent.click(await screen.findByRole('button',{name:'Delete thread'}));
  expect(removed).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'Delete'}));
  await waitFor(() => expect(removed).toHaveBeenCalledWith('local:reply-1'));
  expect(request).toHaveBeenCalledWith('api/threads/reply-1',{method:'DELETE'});
  expect(navigate).toHaveBeenCalled(); expect(screen.queryByRole('alert')).toBeNull();
});
it.each([403,404])('silently removes a stale menu reference (%s)', async status => {
  vi.mocked(request).mockRejectedValue(new ApiError(status,{code:'thread_scope_forbidden',message:'Denied'}));
  const {removed}=mount();
  fireEvent.click(await screen.findByRole('button',{name:'Actions for reply 1'}));
  await waitFor(() => expect(removed).toHaveBeenCalledWith(thread.key));
  expect(screen.queryByRole('dialog')).toBeNull(); expect(screen.queryByText('Denied')).toBeNull();
});
it('names its icons with data-icon for theme packs', async () => {
  mount();
  const trigger = await screen.findByRole('button',{name:'Actions for reply 1'});
  expect(trigger.querySelector('svg')?.getAttribute('data-icon')).toBe('more');
  fireEvent.click(trigger);
  const icon = async (name: string) => (await screen.findByRole('button',{name})).querySelector('svg')?.getAttribute('data-icon');
  expect(await icon('Star thread')).toBe('shortcut');
  expect(await icon('Open thread')).toBe('open');
  expect(await icon('Rename thread')).toBe('rename');
  expect(await icon('Delete thread')).toBe('delete');
});
it('confirms a copy on the item that copied, then closes', async () => {
  const writeText = vi.fn(async () => undefined);
  Object.assign(navigator, { clipboard: { writeText } });
  mount();
  fireEvent.click(await screen.findByRole('button',{name:'Actions for reply 1'}));
  const item = await screen.findByRole('button',{name:'Copy Pockymoe session ID'});
  fireEvent.click(item);
  await waitFor(() => expect(item).toHaveAttribute('data-copy-state','copied'));
  expect(writeText).toHaveBeenCalledWith('reply-1');
  expect(item).toHaveTextContent('Copied');
  // No separate notice at the foot of the menu.
  expect(screen.queryByRole('status')).toBeNull();
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull(), { timeout: 2000 });
});
