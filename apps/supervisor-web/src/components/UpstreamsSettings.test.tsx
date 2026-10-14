import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter, useNavigate } from 'react-router-dom';
import { UpstreamsSettings } from './UpstreamsSettings';
import { relayModeActive, request } from '../lib/api';
vi.mock('../lib/api', () => ({ request: vi.fn(), relayModeActive: vi.fn() }));
beforeEach(() => {
  vi.mocked(request).mockReset();
  vi.mocked(relayModeActive).mockReturnValue(false);
});
const profiles = [
  {
    id: 'p',
    name: 'Work API',
    harness: 'codex',
    baseUrl: 'https://work.test/v1',
    model: 'model-a',
    apiType: 'responses',
    contextWindow: 100000,
    hasApiKey: true,
  },
  {
    id: 'old',
    name: 'Hidden Gemini API',
    harness: 'gemini',
    baseUrl: 'https://gemini.test',
    model: 'gemini',
    apiType: 'responses',
    contextWindow: 100000,
  },
];
it('shows only installed base harnesses and keeps existing upstreams on the selected device', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  vi.mocked(request).mockImplementation(async (path) => {
    if (String(path).endsWith('/harnesses'))
      return [
        { id: 'codex', name: 'OpenAI Codex', base: { installed: true } },
        {
          id: 'claude',
          name: 'Claude Agent',
          base: { path: '/isolated/claude' },
        },
        {
          id: 'deepseek',
          name: 'DSH',
          base: { installed: false },
          adapter: { installed: true },
        },
        { id: 'gemini', name: 'Gemini CLI', base: null },
      ];
    return { profiles, active: { codex: 'p' }, backups: [] };
  });
  render(
    <MemoryRouter initialEntries={['/devices/a/workspaces']}>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  expect(
    await screen.findByRole('button', { name: 'OpenAI Codex' }),
  ).toBeVisible();
  expect(screen.getByRole('button', { name: 'Claude Agent' })).toBeVisible();
  expect(screen.queryByRole('button', { name: 'DSH' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Gemini CLI' })).toBeNull();
  expect(await screen.findByText('Work API')).toBeVisible();
  expect(screen.queryByText('Hidden Gemini API')).toBeNull();
  expect(screen.getByRole('button', { name: 'Using' })).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox', { name: 'Show uninstalled' }));
  expect(screen.getByRole('button', { name: 'DSH' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'DSH' }));
  expect(screen.getByText(/Install this harness/)).toBeVisible();
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Add upstream' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Add upstream' }));
  expect(
    screen.getByRole('option', { name: 'DeepSeek API' }),
  ).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  fireEvent.click(screen.getByRole('checkbox', { name: 'Show uninstalled' }));
  await screen.findByText('Work API');
  fireEvent.change(screen.getByRole('searchbox'), {
    target: { value: 'no-match' },
  });
  expect(screen.queryByText('Work API')).toBeNull();
  expect(screen.getByText('No upstreams match your search.')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Add upstream' }));
  expect(
    screen.getByLabelText('Harness').querySelectorAll('option'),
  ).toHaveLength(1);
  expect(request).toHaveBeenCalledWith(
    'relay/devices/a/api/management/upstreams',
    expect.anything(),
  );
});
it('does not show default harnesses when inventory is empty or unavailable', async () => {
  vi.mocked(request).mockResolvedValue([]);
  render(
    <MemoryRouter>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  expect(await screen.findByText(/No harnesses are installed/)).toBeVisible();
  expect(screen.queryByRole('button', { name: 'OpenAI Codex' })).toBeNull();
  vi.mocked(request).mockRejectedValue(new Error('offline'));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh harnesses' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Unable to check installed harnesses',
  );
  expect(screen.queryByRole('button', { name: 'Add upstream' })).toBeNull();
});
it('ignores an old inventory response after switching devices', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  let resolveA!: (value: unknown) => void;
  vi.mocked(request).mockImplementation(async (path) => {
    if (String(path).includes('/devices/a/'))
      return new Promise((resolve) => {
        resolveA = resolve;
      });
    if (String(path).endsWith('/harnesses'))
      return [
        { id: 'claude', name: 'Claude Agent', base: { installed: true } },
      ];
    return { profiles: [], active: {}, backups: [] };
  });
  function View() {
    const navigate = useNavigate();
    return (
      <>
        <button onClick={() => navigate('/devices/b/workspaces')}>
          Device B
        </button>
        <UpstreamsSettings />
      </>
    );
  }
  render(
    <MemoryRouter initialEntries={['/devices/a/workspaces']}>
      <View />
    </MemoryRouter>,
  );
  await waitFor(() => expect(resolveA).toBeTypeOf('function'));
  fireEvent.click(screen.getByRole('button', { name: 'Device B' }));
  await screen.findByRole('button', { name: 'Claude Agent' });
  resolveA([{ id: 'codex', name: 'OpenAI Codex', base: { installed: true } }]);
  await waitFor(() =>
    expect(request).toHaveBeenCalledWith(
      'relay/devices/b/api/management/upstreams',
      expect.anything(),
    ),
  );
  expect(screen.queryByRole('button', { name: 'OpenAI Codex' })).toBeNull();
});
it('requires a device selection on relay home', () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  render(
    <MemoryRouter initialEntries={['/workspaces']}>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  expect(
    screen.getByText('Open a device to manage its upstreams.'),
  ).toBeVisible();
  expect(request).not.toHaveBeenCalled();
});
