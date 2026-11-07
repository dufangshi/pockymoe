import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { saveEnvironmentConnection, bindThreadEnvironment, fetchThreadEnvironment, createEnvironment } from './environmentApi';
import { environmentConnection } from '../test/environmentFixtures';
import { fetchWorkspaceDocument, saveWorkspaceDocument, fetchWorkspaceSaveOperation, createWorkspaceFile, fetchWorkspaceFileTree, fetchWorkspaceFilePreview, uploadWorkspaceFile, renameWorkspaceNode, deleteWorkspaceNode, downloadWorkspaceFile, buildWorkspaceRawFileUrl } from './api';

const calls: { url: string; init?: RequestInit }[] = [];
beforeEach(() => {
  localStorage.clear(); calls.length = 0; setLocale('en', false);
  vi.stubGlobal('fetch', vi.fn(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), ...(init ? { init } : {}) });
    return new Response(JSON.stringify({ id: 'redacted', configured: true }), { headers: { 'content-type': 'application/json' } });
  }));
});
afterEach(() => { vi.unstubAllGlobals(); localStorage.clear(); });
it('omits a blank write-only key and sends a replacement only in the PUT body', async () => {
  const { id, configured, ...input } = environmentConnection;
  await saveEnvironmentConnection(id, { ...input, apiKey: '' }, null);
  expect(JSON.parse(String(calls[0]!.init!.body))).not.toHaveProperty('apiKey');
  await saveEnvironmentConnection(id, { ...input, apiKey: 'synthetic-key-only' }, null);
  expect(JSON.parse(String(calls[1]!.init!.body)).apiKey).toBe('synthetic-key-only');
  expect(calls[1]!.url).not.toContain('synthetic-key-only');
  expect(JSON.stringify(localStorage)).not.toContain('synthetic-key-only');
});
it('scopes create/read/bind to the device and makes session reset explicit', async () => {
  await fetchThreadEnvironment('thread one', null);
  await createEnvironment({ threadId: 'thread one', connectionId: 'c' }, null);
  await bindThreadEnvironment('thread one', 'env', null);
  await bindThreadEnvironment('thread one', 'env', null, true);
  expect(calls[0]!.url).toContain('api/threads/thread%20one/environment');
  expect(JSON.parse(String(calls[1]!.init!.body))).toEqual({ threadId: 'thread one', connectionId: 'c' });
  expect(JSON.parse(String(calls[2]!.init!.body))).toEqual({ environmentId: 'env' });
  expect(JSON.parse(String(calls[3]!.init!.body))).toEqual({ environmentId: 'env', resetSession: true });
});
it('preserves thread context on every workspace file endpoint without changing the submitted save', async () => {
  const save = { path: 'a.md', operationId: 'op', draftRevision: 1, content: 'hello' } as Parameters<typeof saveWorkspaceDocument>[1];
  await fetchWorkspaceFileTree('w', {}, null, 'thread one');
  await fetchWorkspaceFilePreview('w', { path: 'a.md' }, null, 'thread one');
  await fetchWorkspaceDocument('w', 'a.md', undefined, null, 'thread one');
  await saveWorkspaceDocument('w', save, null, 'thread one');
  await fetchWorkspaceSaveOperation('w', 'op', null, 'thread one');
  await createWorkspaceFile('w', 'b.md', null, 'thread one');
  await renameWorkspaceNode('w', { fromPath: 'a.md', toPath: 'b.md' }, null, 'thread one');
  await deleteWorkspaceNode('w', 'b.md', null, 'thread one');
  await uploadWorkspaceFile('w', { file: new File(['a'], 'a.txt') }, null, 'thread one');
  await downloadWorkspaceFile('w', { path: 'a.md' }, null, undefined, 'thread one');
  expect(calls).toHaveLength(10);
  for (const call of calls) expect(new URL(call.url, location.origin).searchParams.get('threadId')).toBe('thread one');
  expect(new URL(buildWorkspaceRawFileUrl('w', { path: 'a.md' }, null, 'thread one'), location.origin).searchParams.get('threadId')).toBe('thread one');
  expect(JSON.parse(String(calls[3]!.init!.body))).toEqual(save);
});
