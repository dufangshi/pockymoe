import { afterEach, expect, it, vi } from 'vitest';
import { fetchStorageSettings, updateStorageSettings } from './api';

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
