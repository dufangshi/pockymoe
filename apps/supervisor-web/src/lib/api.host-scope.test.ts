import { afterEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ApiError, request } from './api';

afterEach(() => { vi.unstubAllGlobals(); setLocale('en', false); });
it.each(['en', 'zh-CN'] as const)('explains scope-denied 403 responses in %s', async locale => {
  setLocale(locale, false);
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(Response.json({ code: 'thread_scope_forbidden', error: 'thread_scope_forbidden' }, { status: 403 })));
  const error = await request('api/threads', { method: 'POST' }).catch(error => error);
  expect(error).toBeInstanceOf(ApiError);
  if (!(error instanceof ApiError)) throw new Error('Expected API error');
  expect(error.statusCode).toBe(403);
  expect(error.payload.code).toBe('thread_scope_forbidden');
  expect(error.message).toContain('Treer');
  expect(error.message.length).toBeGreaterThan(15);
  expect(error.message).not.toContain('403');
});
