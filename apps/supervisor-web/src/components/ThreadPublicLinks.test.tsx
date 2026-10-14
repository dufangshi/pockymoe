import { webcrypto } from 'node:crypto';
import { StrictMode } from 'react';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ThreadPublicLinks } from './ThreadPublicLinks';
import { request, fetchThreadExportTurns } from '../lib/api';
import { decryptPublicSnapshot } from '../lib/publicLinkCrypto';
import { loadExportSnapshot } from '../lib/transcriptExport';

vi.mock('../lib/api', () => ({ request: vi.fn(), fetchThreadExportTurns: vi.fn() }));
vi.mock('../lib/transcriptExport', () => ({ loadExportSnapshot: vi.fn() }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
beforeEach(() => { localStorage.clear(); Object.defineProperty(globalThis, 'crypto', { configurable: true, value: webcrypto }); vi.clearAllMocks(); vi.mocked(fetchThreadExportTurns).mockResolvedValue({ totalTurnCount: 2, turns: [1, 2].map(n => ({ turnId: `turn-${n}`, turnNumber: n, status: 'completed', startedAt: null, userPromptPreview: `Prompt ${n}` })) }); });

it('waits for explicit creation and shares only selected turns', async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  vi.mocked(loadExportSnapshot).mockResolvedValue({ title: 'Test snapshot', turns: [] } as never);
  let resolveList!: (value: unknown) => void;
  vi.mocked(request).mockImplementation(async (_url, init) => {
    if (init?.method === 'POST') return { id: 'new-link', turnCount: 1, createdAt: '2026-09-19T12:00:00Z' };
    return new Promise((resolve) => { resolveList = resolve; });
  });
  render(<StrictMode><ThreadPublicLinks deviceId="mac" threadId="thread" /></StrictMode>);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create & copy link' })).toBeEnabled());
  expect(loadExportSnapshot).not.toHaveBeenCalled();
  fireEvent.click(screen.getByLabelText('Choose turns'));
  fireEvent.click(screen.getByLabelText('Share turn 1'));
  fireEvent.click(screen.getByRole('button', { name: 'Create & copy link' }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(expect.stringMatching(/\/s\/new-link#k=[\w-]{43}$/)));
  resolveList([]);
  await waitFor(() => expect(screen.getByLabelText('Public share URL')).toHaveValue(writeText.mock.calls[0]![0]));
  expect(vi.mocked(request).mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(1);
  expect(loadExportSnapshot).toHaveBeenCalledWith('thread', { mode: 'selected', turnIds: ['turn-2'] });
  expect(screen.getByRole('status')).toHaveTextContent('Read-only link copied');
  const body = JSON.parse(vi.mocked(request).mock.calls.find(([, init]) => init?.method === 'POST')![1]!.body as string);
  expect(Object.keys(body.snapshot)).toEqual(['version', 'iv', 'ciphertext']);
  expect(body.encryptionKey).toBeUndefined();
  expect(await decryptPublicSnapshot(body.snapshot, new URL(writeText.mock.calls[0]![0]).hash)).toMatchObject({ turns: [] });
});

it('retains the created URL when clipboard access is unavailable', async () => {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
  vi.mocked(loadExportSnapshot).mockResolvedValue({ title: 'Test snapshot', turns: [] } as never);
  vi.mocked(request).mockImplementation(async (_url, init) => init?.method === 'POST'
    ? { id: 'manual-copy', turnCount: 1, createdAt: '2026-09-19T12:00:00Z' } : []);
  render(<ThreadPublicLinks deviceId="mac" threadId="thread" />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create & copy link' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Create & copy link' }));
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Link created'));
  expect((screen.getByLabelText('Public share URL') as HTMLInputElement).value).toMatch(/\/s\/manual-copy#k=[\w-]{43}$/);
});

it('requires explicit live consent and configures the device before publishing its capability', async () => {
  vi.mocked(request).mockImplementation(async (url, init) => {
    if (String(url).endsWith('/publications')) return { token: 'capability', snapshot: { version: 2, iv: 'opaque', ciphertext: 'opaque' } };
    if (init?.method === 'POST') return { id: 'live-link', live: true, turnCount: 2, createdAt: '2026-09-19T12:00:00Z' };
    return [];
  });
  render(<ThreadPublicLinks deviceId="mac" threadId="thread" />);
  const consent = screen.getByRole('checkbox', { name: /Keep updated/ });
  expect(consent).not.toBeChecked();
  fireEvent.click(consent);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create & copy link' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Create & copy link' }));
  await screen.findByLabelText('Public share URL');
  const posts = vi.mocked(request).mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(posts[0]?.[0]).toBe('api/threads/thread/publications');
  expect(JSON.parse(posts[0]![1]!.body as string).turnIds).toEqual(['turn-1', 'turn-2']);
  expect(JSON.parse(posts[0]![1]!.body as string).encryptionKey).toMatch(/^[\w-]{43}$/);
  expect(JSON.parse(posts[1]![1]!.body as string).encryptionKey).toBeUndefined();
  expect(JSON.parse(posts[1]![1]!.body as string).snapshot).toBeUndefined();
  expect(JSON.parse(posts[1]![1]!.body as string).publicationToken).toBe('capability');
  expect(loadExportSnapshot).not.toHaveBeenCalled();
});

it('withholds encrypted URLs whose keys were not kept and preserves legacy copy URLs', async () => {
  vi.mocked(request).mockResolvedValue([
    { id: 'missing-key', version: 2, createdAt: '2026-09-19T12:00:00Z' },
    { id: 'legacy', version: 1, turnCount: 2, createdAt: '2026-09-19T12:00:00Z' },
  ]);
  render(<ThreadPublicLinks deviceId="mac" threadId="thread" />);
  const urls = await screen.findAllByLabelText('Public share URL');
  expect(urls[0]).toHaveValue('Link key is unavailable in this browser.');
  expect(urls[1]).toHaveValue(`${location.origin}/s/legacy`);
  expect(screen.getAllByRole('button', { name: 'Copy share link' })[0]).toBeDisabled();
});

it('rejects older live publishers before forwarding any plaintext to the relay', async () => {
  vi.mocked(request).mockImplementation(async (url, init) => {
    if (String(url).endsWith('/publications')) return { token: 'old-capability', snapshot: { title: 'private', live: true, turns: [] } };
    if (init?.method === 'DELETE') return undefined;
    return [];
  });
  render(<ThreadPublicLinks deviceId="mac" threadId="thread" />);
  fireEvent.click(screen.getByRole('checkbox', { name: /Keep updated/ }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create & copy link' })).toBeEnabled());
  fireEvent.click(screen.getByRole('button', { name: 'Create & copy link' }));
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Update the device'));
  expect(vi.mocked(request).mock.calls.some(([url, init]) => url === 'relay/thread-links' && init?.method === 'POST')).toBe(false);
  expect(request).toHaveBeenCalledWith('api/publications/old-capability', { method: 'DELETE' });
});

it('revokes a persisted live publication through the owner channel as well as the relay', async () => {
  vi.mocked(request).mockImplementation(async (_url, init) => init?.method === 'DELETE' ? undefined : [
    { id: 'revoke-live', version: 2, live: true, publicationToken: 'publication-capability', createdAt: '2026-09-19T12:00:00Z' },
  ]);
  render(<ThreadPublicLinks deviceId="mac" threadId="thread" />);
  fireEvent.click(await screen.findByRole('button', { name: 'Revoke share link' }));
  await waitFor(() => expect(request).toHaveBeenCalledWith('api/publications/publication-capability', { method: 'DELETE' }));
  expect(request).toHaveBeenCalledWith('relay/public-links/revoke-live', { method: 'DELETE' });
  expect(screen.queryByLabelText('Public share URL')).toBeNull();
});
