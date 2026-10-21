import { renderHook } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
const source = vi.hoisted(() => ({ key: 'owner:device-a' }));
vi.mock('../lib/api', () => ({
  ApiError: class extends Error {
    statusCode = 404;
  },
  workspaceResourceScope: () => source.key,
  fetchWorkspaceDocument: vi.fn(),
  saveWorkspaceDocument: vi.fn(),
  fetchWorkspaceSaveOperation: vi.fn(),
  fetchLinkedFile: vi.fn(),
  fetchLinkedFilePreview: vi.fn(),
  buildLinkedFileUrl: vi.fn(),
  buildWorkspaceRawFileUrl: vi.fn(),
  downloadWorkspaceFile: vi.fn(),
  downloadLinkedFile: vi.fn(),
  fetchWorkspaceFilePreview: vi.fn(),
  fetchWorkspaceFileTree: vi.fn(),
  uploadWorkspaceFile: vi.fn(),
  renameWorkspaceNode: vi.fn(),
  deleteWorkspaceNode: vi.fn(),
  createWorkspaceFile: vi.fn(),
}));
import { useThreadWorkspaceAdapter } from './useThreadWorkspaceAdapter';
import * as api from '../lib/api';
describe('workspace adapter source identity', () => {
  it('creates only in the workspace bound to the writable adapter', async () => {
    vi.mocked(api.createWorkspaceFile).mockResolvedValue({ path: 'docs/new.md' });
    const { result } = renderHook(() => useThreadWorkspaceAdapter({ setError: vi.fn(), workspaceId: 'bound-workspace' }));
    await result.current?.createFile?.({ threadId: 'thread', workspaceId: 'untrusted-workspace', path: 'docs/new.md' });
    expect(api.createWorkspaceFile).toHaveBeenCalledWith('bound-workspace', 'docs/new.md', undefined);
  });
  it('changes source when the device changes even if workspace IDs are identical', () => {
    const setError = vi.fn();
    const { result, rerender } = renderHook(() =>
      useThreadWorkspaceAdapter({ setError, workspaceId: 'same-workspace' }),
    );
    const first = result.current;
    expect(first?.resourceScopeKey).toBe('owner:device-a');
    source.key = 'owner:device-b';
    rerender();
    expect(result.current?.resourceScopeKey).toBe('owner:device-b');
    expect(result.current).not.toBe(first);
  });
  it('offers safe reads but no write or receipt methods to a read-only grant', () => {
    const { result } = renderHook(() =>
      useThreadWorkspaceAdapter({
        setError: vi.fn(),
        workspaceId: 'same-workspace',
        access: 'read',
      }),
    );
    expect(result.current?.readDocument).toBeTypeOf('function');
    expect(result.current?.saveDocument).toBeUndefined();
    expect(result.current?.getSaveOperation).toBeUndefined();
    expect(result.current?.writeFile).toBeUndefined();
    expect(result.current?.createFile).toBeUndefined();
    expect(result.current?.textRangeRead).toBe(false);
  });
});
it('captures both device and workspace through focus changes and an in-flight save', async () => {
  const setError = vi.fn();
  const {result,rerender} = renderHook(({deviceId,workspaceId}) => useThreadWorkspaceAdapter({setError,deviceId,workspaceId}), {initialProps:{deviceId:'device-a',workspaceId:'workspace-a'}});
  const a = result.current!;
  rerender({deviceId:'device-b',workspaceId:'workspace-b'});
  const b = result.current!;
  await a.listTree({threadId:'thread-a',workspaceId:'untrusted'});
  await b.listTree({threadId:'thread-b',workspaceId:'untrusted'});
  expect(api.fetchWorkspaceFileTree).toHaveBeenCalledWith('workspace-a',{path:''},'device-a');
  expect(api.fetchWorkspaceFileTree).toHaveBeenCalledWith('workspace-b',{path:''},'device-b');
  const save = {threadId:'thread-a',workspaceId:'untrusted',path:'a.md',operationId:'op',draftRevision:1,content:'draft'};
  await a.saveDocument!(save as never);
  expect(api.saveWorkspaceDocument).toHaveBeenCalledWith('workspace-a',expect.objectContaining({path:'a.md',operationId:'op'}),'device-a');
  await b.createFile!({threadId:'thread-b',workspaceId:'untrusted',path:'b.md'});
  expect(api.createWorkspaceFile).toHaveBeenCalledWith('workspace-b','b.md','device-b');
});
it('keeps a thread or workspace share inside its workspace root', async () => {
  vi.mocked(api.fetchWorkspaceFileTree).mockClear();
  vi.mocked(api.fetchWorkspaceFilePreview).mockClear();
  vi.mocked(api.fetchLinkedFilePreview).mockClear();
  vi.mocked(api.downloadWorkspaceFile).mockClear();
  const { result } = renderHook(() => useThreadWorkspaceAdapter({
    setError: vi.fn(), workspaceId: 'shared-ws', deviceId: 'device', access: 'write',
    // Even if a caller offers linked files, a confined share never reads them.
    allowLinkedFiles: true, confined: true,
  }));
  const adapter = result.current!;
  // The tree starts at the workspace root.
  await adapter.listTree({ threadId: 't', workspaceId: 'shared-ws' });
  expect(api.fetchWorkspaceFileTree).toHaveBeenCalledWith('shared-ws', { path: '' }, 'device');
  expect(adapter.statLinkedFile).toBeUndefined();
  for (const path of ['/etc', '..', 'src/../../home']) {
    await expect(adapter.listTree({ threadId: 't', workspaceId: 'shared-ws', path })).rejects.toThrow('Only files in this shared workspace are available.');
  }
  for (const path of ['/etc/passwd', '../outside.txt', 'C:/Users/me/a.txt']) {
    await expect(adapter.readFile({ threadId: 't', workspaceId: 'shared-ws', path })).rejects.toThrow(/shared workspace/);
    await expect(adapter.readRawFile!({ threadId: 't', workspaceId: 'shared-ws', path })).rejects.toThrow(/shared workspace/);
    expect(adapter.getRawFileUrl!({ threadId: 't', workspaceId: 'shared-ws', path })).toBe('data:,');
    expect(() => adapter.saveDocument!({ threadId: 't', workspaceId: 'shared-ws', path, operationId: 'op', draftRevision: 1 } as never)).toThrow(/shared workspace/);
  }
  expect(api.fetchWorkspaceFileTree).toHaveBeenCalledTimes(1);
  expect(api.fetchWorkspaceFilePreview).not.toHaveBeenCalled();
  expect(api.fetchLinkedFilePreview).not.toHaveBeenCalled();
  expect(api.downloadWorkspaceFile).not.toHaveBeenCalled();
  await adapter.readFile({ threadId: 't', workspaceId: 'shared-ws', path: 'docs/readme.md' });
  expect(api.fetchWorkspaceFilePreview).toHaveBeenCalledWith('shared-ws', { path: 'docs/readme.md' }, 'device');
});
