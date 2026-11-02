import { afterEach, expect, it, vi } from 'vitest';
import { fetchContextInspectionSettings, fetchStorageSettings, fetchThreadContext, updateContextInspectionSettings, updateStorageSettings } from './api';

afterEach(() => { localStorage.clear(); vi.unstubAllGlobals(); });
const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { 'content-type': 'application/json' } });

it('reads the settings of a Supervisor that only knows the output retention', async () => {
  vi.stubGlobal('fetch', vi.fn(async () => json({ commandOutputRetentionDays: 3 })));
  expect(await fetchStorageSettings(null)).toEqual({ retentionDays: 3, commandOutput: true, commandText: 'keep', commandTextChars: 512, fileDiffs: false });
});

it('saves the age under the name every Supervisor accepts', async () => {
  const fetch = vi.fn(async (_url: string, init?: RequestInit) => json(JSON.parse(String(init?.body))));
  vi.stubGlobal('fetch', fetch);
  const saved = await updateStorageSettings({ retentionDays: 7, commandOutput: false, commandText: 'remove', commandTextChars: 64, fileDiffs: true }, null);
  expect(JSON.parse(String(fetch.mock.calls[0]![1]!.body))).toEqual({ commandOutputRetentionDays: 7, commandOutput: false, commandText: 'remove', commandTextChars: 64, fileDiffs: true });
  expect(saved).toEqual({ retentionDays: 7, commandOutput: false, commandText: 'remove', commandTextChars: 64, fileDiffs: true });
});

it('asks for a thread\'s context analysis, and treats a Supervisor without it as total-only', async () => {
  const snapshot = { enabled: true, supported: true, availability: 'available', snapshot: null };
  const fetchMock = vi.fn(async () => json(snapshot));
  vi.stubGlobal('fetch', fetchMock);
  expect(await fetchThreadContext('t 1', { detail: 'full', refresh: true }, null)).toEqual(snapshot);
  expect(String((fetchMock.mock.calls as unknown as [string][])[0]![0])).toContain('api/threads/t%201/context?detail=full&refresh=true');

  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ code: 'not_found', message: 'gone' }), { status: 404, headers: { 'content-type': 'application/json' } })));
  expect(await fetchThreadContext('t', {}, null)).toMatchObject({ enabled: false, supported: false, snapshot: null });
});

it('reads and saves the context analysis setting as the Supervisor expects', async () => {
  const fetchMock = vi.fn(async () => json({ enabled: true }));
  vi.stubGlobal('fetch', fetchMock);
  expect(await fetchContextInspectionSettings(null)).toEqual({ enabled: true });
  expect(await updateContextInspectionSettings({ enabled: true }, null)).toEqual({ enabled: true });
  const [path, init] = (fetchMock.mock.calls as unknown as [string, RequestInit][])[1]!;
  expect(String(path)).toContain('api/context-inspection/settings');
  expect(init.method).toBe('PUT');
  expect(JSON.parse(String(init.body))).toEqual({ enabled: true });
});
