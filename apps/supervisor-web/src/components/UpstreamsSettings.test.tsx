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
const inventory = [
  { id: 'codex', name: 'OpenAI Codex', base: { installed: true } },
  { id: 'claude', name: 'Claude Agent', base: { path: '/isolated/claude' } },
  {
    id: 'deepseek',
    name: 'DSH',
    base: { installed: false },
    adapter: { installed: true },
  },
  { id: 'gemini', name: 'Gemini CLI', base: null },
];
const options = (select: HTMLElement) =>
  within(select)
    .getAllByRole('option')
    .map((o) => o.textContent);

it('lists connections on their own and applies them per installed harness below', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  vi.mocked(request).mockImplementation(async (path) => {
    if (String(path).endsWith('/harnesses')) return inventory;
    if (String(path).endsWith('/management/connections')) return catalog;
    return { profiles: [], active: {}, backups: [] };
  });
  render(
    <MemoryRouter initialEntries={['/devices/a/workspaces']}>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  // Connections come first and carry no harness actions.
  const work = await screen.findByRole('listitem', { name: 'Work API' });
  expect(await within(work).findByText('OpenAI Codex')).toBeVisible();
  expect(within(work).queryByRole('button', { name: /apply/i })).toBeNull();
  expect(screen.queryByRole('group', { name: 'Choose harness' })).toBeNull();

  const harnesses = screen.getByRole('region', { name: 'Use in harnesses' });
  const codex = await within(harnesses).findByRole('listitem', {
    name: 'OpenAI Codex',
  });
  expect(within(codex).getByTestId('harness-current')).toHaveTextContent(
    'Work API · model-a',
  );
  expect(
    options(within(codex).getByLabelText('Connection for OpenAI Codex')),
  ).toEqual(['Choose a connection', 'Work API']);
  expect(
    within(codex).getByRole('button', {
      name: 'Reapply Work API to OpenAI Codex',
    }),
  ).toBeEnabled();
  const claude = within(harnesses).getByRole('listitem', {
    name: 'Claude Agent',
  });
  expect(
    options(within(claude).getByLabelText('Connection for Claude Agent')),
  ).toEqual(['Choose a connection', 'Anthropic Direct']);
  // Uninstalled harnesses stay out of the way until asked for.
  expect(within(harnesses).queryByRole('listitem', { name: 'DSH' })).toBeNull();
  fireEvent.click(screen.getByRole('checkbox', { name: 'Show uninstalled' }));
  const dsh = within(harnesses).getByRole('listitem', { name: 'DSH' });
  expect(within(dsh).getByText('Not installed')).toBeVisible();
  fireEvent.change(within(dsh).getByLabelText('Connection for DSH'), {
    target: { value: 'claude-only' },
  });
  expect(
    within(dsh).getByRole('button', { name: 'Apply Anthropic Direct to DSH' }),
  ).toBeDisabled();
  // Native tools stay available per harness, but only load when opened.
  expect(request).not.toHaveBeenCalledWith(
    'relay/devices/a/api/management/upstreams',
    expect.anything(),
  );
  fireEvent.click(
    within(codex).getByRole('button', {
      name: 'OpenAI Codex native configuration',
    }),
  );
  expect(
    await within(codex).findByRole('button', { name: 'Import config' }),
  ).toBeVisible();
  expect(request).toHaveBeenCalledWith(
    'relay/devices/a/api/management/connections',
    expect.anything(),
  );
});

it('adds connections without any harness and when the harness inventory fails', async () => {
  let inventoryResult: () => Promise<unknown> = async () => [];
  let saved = false;
  const lab = { ...catalog.connections[1], id: 'new', name: 'Lab' };
  vi.mocked(request).mockImplementation(async (path, init) => {
    if (String(path).endsWith('/harnesses')) return inventoryResult();
    if (
      String(path).endsWith('/management/connections') &&
      init?.method === 'POST'
    ) {
      saved = true;
      return lab;
    }
    return {
      ...catalog,
      connections: saved ? [lab] : [],
      bindings: {},
    };
  });
  render(
    <MemoryRouter>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
  expect(await screen.findByText(/No harnesses are installed/)).toBeVisible();
  expect(await screen.findByText(/No connections yet/)).toBeVisible();
  inventoryResult = async () => {
    throw new Error('offline');
  };
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  const harnesses = screen.getByRole('region', { name: 'Use in harnesses' });
  expect(await within(harnesses).findByRole('alert')).toHaveTextContent(
    'Unable to check installed harnesses',
  );
  // The inventory failure is confined to the harness section.
  const add = screen.getByRole('button', { name: 'Add connection' });
  await waitFor(() => expect(add).toBeEnabled());
  fireEvent.click(add);
  const dialog = await screen.findByRole('dialog', { name: 'Add connection' });
  fireEvent.change(within(dialog).getByLabelText('Name'), {
    target: { value: 'Lab' },
  });
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://lab.test/v1' },
  });
  fireEvent.change(within(dialog).getByLabelText('API key'), {
    target: { value: 'k' },
  });
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  );
  expect(await screen.findByRole('listitem', { name: 'Lab' })).toBeVisible();
  expect(
    vi
      .mocked(request)
      .mock.calls.some(([path]) => String(path).includes('/apply')),
  ).toBe(false);
});

it('ignores an old inventory response after switching devices', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  let resolveA!: (value: unknown) => void;
  vi.mocked(request).mockImplementation(async (path) => {
    if (
      String(path).includes('/devices/a/') &&
      String(path).endsWith('/harnesses')
    )
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
  await screen.findByRole('listitem', { name: 'Claude Agent' });
  resolveA([{ id: 'codex', name: 'OpenAI Codex', base: { installed: true } }]);
  await waitFor(() =>
    expect(request).toHaveBeenCalledWith(
      'relay/devices/b/api/management/connections',
      expect.anything(),
    ),
  );
  expect(screen.queryByRole('listitem', { name: 'OpenAI Codex' })).toBeNull();
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
