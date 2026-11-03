import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ClientUpdateButton } from './ClientUpdateButton';
import type { PockymoeHost, UpdateState } from './host';
const ready: UpdateState = { status: 'ready', version: '0.3.0', releaseUrl: 'https://github.com/dufangshi/pockymoe-app/releases/tag/v0.3.0' };
function fixture(initial: UpdateState = ready) {
  let listener: (state: UpdateState) => void = () => {};
  const unsubscribe = vi.fn(), install = vi.fn(), openExternal = vi.fn();
  const host = { updates: { state: vi.fn(async () => initial), onChange: vi.fn((fn: typeof listener) => { listener = fn; return unsubscribe; }), install }, shell: { openExternal } } as unknown as PockymoeHost;
  return { host, install, openExternal, unsubscribe, emit: (state: UpdateState) => act(() => listener(state)) };
}
beforeEach(() => setLocale('zh-CN', false));
afterEach(() => vi.restoreAllMocks());
it('shows nothing in browsers, older clients, disabled hosts and while downloading', async () => {
  const f = fixture({ status: 'downloading', version: '0.3.0' });
  const view = render(<ClientUpdateButton host={null} enabled={false} />);
  expect(view.container).toBeEmptyDOMElement();
  view.rerender(<ClientUpdateButton host={{} as PockymoeHost} enabled />); expect(view.container).toBeEmptyDOMElement();
  view.rerender(<ClientUpdateButton host={f.host} enabled={false} />); expect(view.container).toBeEmptyDOMElement(); expect(f.host.updates!.state).not.toHaveBeenCalled();
  view.rerender(<ClientUpdateButton host={f.host} enabled />); await act(async () => {}); expect(view.container).toBeEmptyDOMElement();
});
it('installs with no arguments or confirmation and shows the existing spinner immediately', async () => {
  const f = fixture(); render(<ClientUpdateButton host={f.host} enabled />);
  const button = await screen.findByRole('button', { name: '更新到 0.3.0' });
  expect(button).toHaveAttribute('title', '更新到 0.3.0'); expect(button.querySelector('svg.lucide-arrow-up')).not.toBeNull();
  fireEvent.click(button); expect(f.install).toHaveBeenCalledWith(); expect(f.install).toHaveBeenCalledTimes(1);
  expect(button).toBeDisabled(); expect(button).toHaveAttribute('aria-busy', 'true'); expect(button.querySelector('.composer-send-spinner')).not.toBeNull();
});
it('retains the icon on failure and offers Retry and Go to download in ErrorMark', async () => {
  const f = fixture(); render(<ClientUpdateButton host={f.host} enabled />);
  fireEvent.click(await screen.findByRole('button', { name: '更新到 0.3.0' }));
  f.emit({ ...ready, status: 'error', error: '磁盘空间不足。' });
  expect(screen.getByRole('button', { name: '更新到 0.3.0' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: '查看失败原因' })); expect(screen.getByRole('dialog')).toHaveTextContent('磁盘空间不足。');
  fireEvent.click(screen.getByRole('button', { name: '前往下载' })); expect(f.openExternal).toHaveBeenCalledWith(ready.releaseUrl);
  fireEvent.click(screen.getByRole('button', { name: '查看失败原因' })); fireEvent.click(screen.getByRole('button', { name: '重试' })); expect(f.install).toHaveBeenCalledTimes(2);
});
it('does not offer an arbitrary host error URL as a download recovery', async () => {
  const f = fixture({ ...ready, status: 'error', releaseUrl: 'https://example.invalid/arbitrary' }); render(<ClientUpdateButton host={f.host} enabled />);
  fireEvent.click(await screen.findByRole('button', { name: '查看失败原因' })); expect(screen.queryByRole('button', { name: '前往下载' })).not.toBeInTheDocument();
});
it('subscribes before loading, preserves newer events, and unsubscribes on unmount', async () => {
  const f = fixture(); let resolve!: (state: UpdateState) => void; f.host.updates!.state = () => new Promise(done => { resolve = done; });
  const view = render(<ClientUpdateButton host={f.host} enabled />);
  f.emit(ready); expect(screen.getByRole('button', { name: '更新到 0.3.0' })).toBeInTheDocument();
  await act(async () => resolve({ status: 'idle' })); expect(screen.getByRole('button', { name: '更新到 0.3.0' })).toBeInTheDocument();
  view.unmount(); expect(f.unsubscribe).toHaveBeenCalledOnce();
});
it('follows the shared language and handles a synchronous install failure', async () => {
  const f = fixture(); f.install.mockImplementation(() => { throw Error('failure'); }); render(<ClientUpdateButton host={f.host} enabled />);
  await screen.findByRole('button', { name: '更新到 0.3.0' }); act(() => setLocale('en', false));
  const button = screen.getByRole('button', { name: 'Update to 0.3.0' }); fireEvent.click(button);
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('The update could not be installed.'));
});

it('keeps download recovery but disables installation and Retry after the native attempt limit', async () => {
  const f = fixture({ ...ready, status: 'error', retryAllowed: false }); render(<ClientUpdateButton host={f.host} enabled />);
  const icon = await screen.findByRole('button', { name: '更新到 0.3.0' }); expect(icon).toBeDisabled(); fireEvent.click(icon); expect(f.install).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: '查看失败原因' })); expect(screen.getByRole('button', { name: '重试' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: '前往下载' })); expect(f.openExternal).toHaveBeenCalledWith(ready.releaseUrl);
});

it('never shows an icon for an unverified higher version kept in idle state', async () => {
  const f = fixture({ status: 'idle', version: '0.3.0' }); const view = render(<ClientUpdateButton host={f.host} enabled />);
  await act(async () => {}); expect(view.container).toBeEmptyDOMElement(); expect(f.install).not.toHaveBeenCalled(); expect(f.openExternal).not.toHaveBeenCalled();
});
