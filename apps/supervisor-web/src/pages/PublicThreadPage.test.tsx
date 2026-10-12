import { webcrypto } from 'node:crypto';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import fixture from '../../../../tests/fixtures/public-link-v2.json';
import { PublicThreadPage } from './PublicThreadPage';

vi.mock('@pockymoe/thread-ui', () => ({ PublicTranscript: ({ snapshot }: { snapshot: { title: string; theme: string; stale?: boolean } }) => <div data-testid="transcript">{snapshot.title} {snapshot.theme} {snapshot.stale ? 'stale' : ''}</div> }));
beforeEach(() => {
  Object.defineProperty(globalThis, 'crypto', { configurable: true, value: webcrypto });
  history.replaceState(null, '', '/');
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function page(value: unknown, hash = '') {
  history.replaceState(null, '', `/${hash}`);
  const fetch = vi.fn().mockResolvedValue({ ok: true, json: async () => value });
  vi.stubGlobal('fetch', fetch);
  render(<MemoryRouter initialEntries={['/s/test']}><Routes><Route path="/s/:id" element={<PublicThreadPage />} /></Routes></MemoryRouter>);
  return fetch;
}
it('renders version 1 plaintext without a key', async () => {
  page({ ...fixture.snapshot, version: 1, live: false });
  expect(await screen.findByTestId('transcript')).toHaveTextContent(fixture.snapshot.title);
});
it('decrypts version 2 in the browser without sending the fragment to the relay', async () => {
  const fetch = page({ ...fixture.envelope, stale: true }, `#k=${fixture.key}`);
  expect(await screen.findByTestId('transcript')).toHaveTextContent(`${fixture.snapshot.title} light stale`);
  expect(fetch).toHaveBeenCalledWith('/relay/public-links/test', expect.objectContaining({ credentials: 'omit', cache: 'no-store' }));
  expect(fetch.mock.calls[0]![0]).not.toContain('#');
});
it('explains missing and wrong keys and stops retrying until the fragment changes', async () => {
  const fetch = page(fixture.envelope);
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('missing its key'));
  expect(screen.queryByTestId('transcript')).toBeNull();
  history.replaceState(null, '', '/#k=AA');
  window.dispatchEvent(new HashChangeEvent('hashchange'));
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('link key is wrong'));
  history.replaceState(null, '', `/#k=${fixture.key}`);
  window.dispatchEvent(new HashChangeEvent('hashchange'));
  expect(await screen.findByTestId('transcript')).toHaveTextContent(fixture.snapshot.title);
  expect(fetch).toHaveBeenCalledTimes(3);
});
