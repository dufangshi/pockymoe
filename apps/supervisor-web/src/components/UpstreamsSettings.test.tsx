import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter, useNavigate } from 'react-router-dom';
import { UpstreamsSettings } from './UpstreamsSettings';
import { relayModeActive, request } from '../lib/api';
vi.mock('../lib/api', () => ({
  request: vi.fn(),
  relayModeActive: vi.fn(),
  ApiError: class extends Error {},
}));
beforeEach(() => {
  vi.mocked(request).mockReset();
  vi.mocked(relayModeActive).mockReturnValue(false);
});
const catalog = {
  connections: [
    {
      id: 'work',
      name: 'Work API',
      protocol: 'openai_responses',
      baseUrl: 'https://work.test/v1',
      model: 'model-a',
      hasApiKey: true,
      revision: 'r1',
      local: true,
      sources: [],
    },
    {
      id: 'claude-only',
      name: 'Anthropic Direct',
      protocol: 'anthropic',
      baseUrl: 'https://anthropic.test',
      model: 'sonnet',
      hasApiKey: true,
      revision: 'r1',
      local: true,
      sources: [],
    },
  ],
  bindings: { codex: { connectionId: 'work', model: 'model-a' } },
  deviceId: 'a',
  catalogAvailable: true,
};
it('shows only installed base harnesses and applies connections in the selected harness context', async () => {
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
    if (String(path).endsWith('/management/connections')) return catalog;
    return { profiles: [], active: {}, backups: [] };
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
  expect(
    await screen.findByRole('listitem', { name: 'Work API' }),
  ).toBeVisible();
  expect(screen.getByTestId('harness-binding')).toHaveTextContent(
    'OpenAI Codex usesWork API· model-a',
  );
  const work = screen.getByRole('listitem', { name: 'Work API' });
  expect(within(work).getByText('In use')).toBeVisible();
  const anthropic = screen.getByRole('listitem', { name: 'Anthropic Direct' });
  expect(within(anthropic).getByText('Not for this harness')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Claude Agent' }));
  expect(
    await within(anthropic).findByRole('button', {
      name: 'Apply to Claude Agent',
    }),
  ).toBeEnabled();
  expect(within(work).queryByRole('button', { name: /Apply to/ })).toBeNull();
  fireEvent.click(screen.getByRole('checkbox', { name: 'Show uninstalled' }));
  fireEvent.click(screen.getByRole('button', { name: 'DSH' }));
  expect(screen.getByText(/Install this harness/)).toBeVisible();
  expect(
    within(
      screen.getByRole('listitem', { name: 'Anthropic Direct' }),
    ).getByRole('button', { name: 'Apply to DSH' }),
  ).toBeDisabled();
  // Native tools stay available, but only load when opened.
  expect(request).not.toHaveBeenCalledWith(
    'relay/devices/a/api/management/upstreams',
    expect.anything(),
  );
  fireEvent.click(screen.getByText('DSH native configuration'));
  expect(
    await screen.findByRole('button', { name: 'Import config' }),
  ).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Add upstream' })).toBeNull();
  expect(request).toHaveBeenCalledWith(
    'relay/devices/a/api/management/connections',
    expect.anything(),
  );
});
it('does not show default harnesses when inventory is empty or unavailable', async () => {
  let inventory: () => Promise<unknown> = async () => [];
  vi.mocked(request).mockImplementation(async (path) =>
    String(path).endsWith('/harnesses')
      ? inventory()
      : { ...catalog, connections: [], bindings: {} },
  );
  render(
    <MemoryRouter>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  expect(await screen.findByText(/No harnesses are installed/)).toBeVisible();
  expect(screen.queryByRole('button', { name: 'OpenAI Codex' })).toBeNull();
  // Connections stay usable for plugins without any harness.
  expect(await screen.findByText(/No connections yet/)).toBeVisible();
  inventory = async () => {
    throw new Error('offline');
  };
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Unable to check installed harnesses',
  );
  expect(screen.queryByRole('button', { name: /Apply to/ })).toBeNull();
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
    return { ...catalog, connections: [], bindings: {} };
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
      'relay/devices/b/api/management/connections',
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
