import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import {
  builtinFrontendPlugins,
  CONTENT_QUALITY_PLUGIN_ID,
} from '@pockymoe/thread-ui/builtin-plugins';
import type { UpstreamConnectionPickerProps } from '@pockymoe/thread-ui';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { ApiError, request } from '../lib/api';
import { ConnectionPicker } from './ConnectionPicker';
import { UpstreamsSettings } from './UpstreamsSettings';

vi.mock('../lib/api', () => ({
  request: vi.fn(),
  relayModeActive: () => false,
  ApiError: class ApiError extends Error {
    constructor(
      readonly statusCode: number,
      readonly payload: { message: string; code?: string },
    ) {
      super(payload.message);
    }
  },
}));

type Call = { path: string; method: string; body: Record<string, unknown> };
let calls: Call[] = [];
function server(route: (call: Call) => unknown) {
  calls = [];
  vi.mocked(request).mockImplementation(async (path, init) => {
    const call = {
      path: String(path),
      method: init?.method ?? 'GET',
      body: init?.body ? JSON.parse(String(init.body)) : undefined,
    };
    // The harness inventory is page plumbing, not part of what tests assert.
    if (call.path.endsWith('/management/harnesses')) return inventory;
    calls.push(call);
    return route(call);
  });
}
const inventory = [
  { id: 'codex', name: 'Codex', base: { installed: true } },
  { id: 'claude', name: 'Claude Agent', base: { installed: true } },
];
/** Settings > Upstreams: connections first, harness usage below. */
function renderPage() {
  render(
    <MemoryRouter>
      <UpstreamsSettings />
    </MemoryRouter>,
  );
}
/** A harness row in the "Use in harnesses" section. */
async function harnessRow(name: string) {
  const section = await screen.findByRole('region', {
    name: 'Use in harnesses',
  });
  return within(section).findByRole('listitem', { name });
}
function fail(status: number, code?: string, message = 'failed') {
  return new (
    ApiError as unknown as new (
      s: number,
      p: { message: string; code?: string },
    ) => Error
  )(status, { message, ...(code ? { code } : {}) });
}
const local = (over: Record<string, unknown> = {}) => ({
  id: 'work',
  name: 'Work API',
  protocol: 'openai_responses',
  baseUrl: 'https://work.test/v1',
  model: 'model-a',
  hasApiKey: true,
  revision: 'r1',
  local: true,
  sources: [],
  ...over,
});
const catalog = (
  connections: unknown[],
  over: Record<string, unknown> = {},
) => ({
  connections,
  bindings: {},
  deviceId: 'desk',
  catalogAvailable: true,
  ...over,
});

beforeEach(() => {
  vi.mocked(request).mockReset();
});

/** Edit, Test and Remove sit in the row's More actions menu. */
async function openEdit(name: string) {
  fireEvent.click(
    await screen.findByRole('button', { name: `More actions for ${name}` }),
  );
  fireEvent.click(screen.getByRole('menuitem', { name: `Edit ${name}` }));
}

it('saves a new connection without applying it and keeps a manual model when discovery fails', async () => {
  let saved = false;
  server(({ path, method, body }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog(saved ? [local({ id: 'lab', name: 'Lab' })] : []);
    if (path === 'api/management/connections/models')
      throw fail(502, undefined, 'upstream refused');
    if (path === 'api/management/connections' && method === 'POST') {
      saved = true;
      return local({ ...body, id: 'lab', revision: 'r1' });
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  expect(await screen.findByText(/No connections yet/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Add connection' }));
  const dialog = await screen.findByRole('dialog', { name: 'Add connection' });
  fireEvent.change(within(dialog).getByLabelText('Name'), {
    target: { value: 'Lab' },
  });
  fireEvent.change(within(dialog).getByLabelText('API format'), {
    target: { value: 'openai_chat' },
  });
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://lab.test/v1' },
  });
  fireEvent.change(within(dialog).getByLabelText('API key'), {
    target: { value: 'test-key' },
  });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Find models' }));
  expect(
    await within(dialog).findByText(
      "Couldn't list models (upstream refused). Type the model ID instead.",
    ),
  ).toBeVisible();
  fireEvent.change(within(dialog).getByLabelText('Default model'), {
    target: { value: 'manual-model' },
  });
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  );
  expect(
    await screen.findByText('Lab saved. Nothing was applied.'),
  ).toBeVisible();
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(
    calls.find((c) => c.method === 'POST' && c.path.endsWith('/connections')),
  ).toMatchObject({
    body: {
      name: 'Lab',
      protocol: 'openai_chat',
      baseUrl: 'https://lab.test/v1',
      apiKey: 'test-key',
      model: 'manual-model',
    },
  });
  expect(calls.find((c) => c.path.endsWith('/models'))?.body).toMatchObject({
    baseUrl: 'https://lab.test/v1',
    apiKey: 'test-key',
  });
  expect(calls.some((c) => c.path.includes('/apply'))).toBe(false);
});

it('applies a local connection only through the explicit Apply dialog and survives a busy harness', async () => {
  let applied = 0;
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog(
        [
          local(),
          local({ id: 'gem', name: 'Gemini Direct', protocol: 'gemini' }),
        ],
        {
          bindings: applied
            ? { codex: { connectionId: 'work', model: 'model-b' } }
            : {},
          // The Supervisor's adapter map wins over the built-in fallback.
          harnessProtocols: { codex: ['openai_responses', 'gemini'] },
        },
      );
    if (path === 'api/management/connections/work/models')
      return {
        models: [{ id: 'model-a' }, { id: 'model-b', name: 'Model B' }],
      };
    if (path === 'api/management/connections/work/apply') {
      if (!applied++) throw fail(409, 'harness_busy');
      return { ok: true, profileId: 'p1' };
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  const row = await screen.findByRole('listitem', { name: 'Work API' });
  expect(within(row).queryByRole('button', { name: /^Sync/ })).toBeNull();
  // Connection rows never apply; harness rows do, one explicit action each.
  expect(within(row).queryByRole('button', { name: /apply/i })).toBeNull();
  const codexRow = await harnessRow('Codex');
  expect(within(codexRow).getByTestId('harness-current')).toHaveTextContent(
    'its own sign-in or native configuration',
  );
  const pick = within(codexRow).getByLabelText('Connection for Codex');
  // The Supervisor's adapter map wins over the built-in fallback.
  expect(
    within(pick)
      .getAllByRole('option')
      .map((o) => o.textContent),
  ).toEqual(['Choose a connection', 'Work API', 'Gemini Direct']);
  expect(
    within(codexRow).getByRole('button', { name: 'Apply' }),
  ).toBeDisabled();
  fireEvent.change(pick, { target: { value: 'work' } });
  const apply = within(codexRow).getByRole('button', {
    name: 'Apply Work API to Codex',
  });
  // Short visible label; the accessible name carries connection and harness.
  expect(apply).toHaveTextContent(/^Apply$/);
  fireEvent.click(apply);
  const dialog = await screen.findByRole('dialog', {
    name: 'Apply Work API to Codex',
  });
  const choose = await within(dialog).findByLabelText(
    'Choose a discovered model',
  );
  fireEvent.change(choose, { target: { value: 'model-b' } });
  expect(within(dialog).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'The harness is busy',
  );
  expect(within(dialog).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
  expect(
    await screen.findByText('Codex now uses Work API · model-b.'),
  ).toBeVisible();
  expect(
    calls.filter((c) => c.path.endsWith('/apply')).map((c) => c.body),
  ).toEqual([
    { harness: 'codex', model: 'model-b' },
    { harness: 'codex', model: 'model-b' },
  ]);
  await waitFor(() =>
    expect(within(codexRow).getByTestId('harness-current')).toHaveTextContent(
      'Work API · model-b',
    ),
  );
  // The connection row now shows where it is used.
  expect(
    within(screen.getByRole('listitem', { name: 'Work API' })).getByText(
      'Codex',
    ),
  ).toBeVisible();
  expect(calls.some((c) => c.path.includes('/sync'))).toBe(false);
});

it('syncs a remote connection in two calls without applying it, with actionable failure and retry', async () => {
  let synced = false;
  let authorizations = 0;
  const remote = {
    id: 'shared',
    name: 'Shared Anthropic',
    protocol: 'anthropic',
    revision: 'r7',
    local: false,
    sources: [
      { deviceId: 'laptop', name: 'Laptop', online: true, revision: 'r7' },
    ],
  };
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        synced
          ? local({
              ...remote,
              local: true,
              baseUrl: 'https://a.test',
              model: 'm',
            })
          : remote,
      ]);
    if (
      path ===
      'relay/devices/laptop/api/management/connections/shared/transfer-authorizations'
    ) {
      if (!authorizations++) throw fail(403, 'peer_access_disabled');
      return { token: 'one-time', expiresAt: '2026-10-16T22:00:00Z' };
    }
    if (path === 'api/management/connections/shared/sync') {
      synced = true;
      return local({ ...remote, local: true });
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  const row = await screen.findByRole('listitem', { name: 'Shared Anthropic' });
  expect(within(row).getByText('Anthropic · On Laptop')).toBeVisible();
  // A remote-only connection cannot be chosen for a harness until it is synced.
  expect(
    within(await harnessRow('Claude Agent')).queryByRole('option', {
      name: 'Shared Anthropic',
    }),
  ).toBeNull();
  expect(
    within(row).queryByRole('button', { name: /^(Re)?apply/i }),
  ).toBeNull();
  expect(within(row).queryByRole('button', { name: /Edit/ })).toBeNull();
  fireEvent.click(
    within(row).getByRole('button', {
      name: 'Sync Shared Anthropic to this device',
    }),
  );
  expect(await within(row).findByRole('alert')).toHaveTextContent(
    'Device-to-device access is off',
  );
  expect(calls.some((c) => c.path.endsWith('/sync'))).toBe(false);
  fireEvent.click(
    within(row).getByRole('button', {
      name: 'Sync Shared Anthropic to this device',
    }),
  );
  expect(
    await screen.findByText('Shared Anthropic is now on this device.'),
  ).toBeVisible();
  const posts = calls.filter((c) => c.method === 'POST');
  expect(posts.map((c) => [c.path, c.body])).toEqual([
    [
      'relay/devices/laptop/api/management/connections/shared/transfer-authorizations',
      { targetDeviceId: 'desk', revision: 'r7' },
    ],
    [
      'relay/devices/laptop/api/management/connections/shared/transfer-authorizations',
      { targetDeviceId: 'desk', revision: 'r7' },
    ],
    [
      'api/management/connections/shared/sync',
      { sourceDeviceId: 'laptop', revision: 'r7', token: 'one-time' },
    ],
  ]);
  // Only now can Claude Agent choose it, and applying is still a separate step.
  const claudeRow = await harnessRow('Claude Agent');
  await waitFor(() =>
    expect(
      within(claudeRow).getByRole('option', { name: 'Shared Anthropic' }),
    ).toBeInTheDocument(),
  );
  expect(calls.some((c) => c.path.includes('/apply'))).toBe(false);
});

it('blocks offline sources and makes the user choose between differing versions without overwriting a changed local copy', async () => {
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        {
          id: 'gone',
          name: 'Offline One',
          protocol: 'openai_chat',
          revision: 'r1',
          local: false,
          sources: [
            { deviceId: 'nas', name: 'NAS', online: false, revision: 'r1' },
          ],
        },
        local({
          id: 'dup',
          name: 'Split',
          revision: 'r1',
          conflict: true,
          sources: [
            {
              deviceId: 'laptop',
              name: 'Laptop',
              online: true,
              revision: 'r2',
            },
            {
              deviceId: 'tablet',
              name: 'Tablet',
              online: true,
              revision: 'r3',
            },
          ],
        }),
      ]);
    if (path.endsWith('/transfer-authorizations')) return { token: 't' };
    if (path.endsWith('/dup/sync')) throw fail(409, 'sync_conflict');
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  const offline = await screen.findByRole('listitem', { name: 'Offline One' });
  expect(within(offline).getByText('Offline')).toBeVisible();
  expect(within(offline).getByRole('button', { name: /^Sync/ })).toBeDisabled();
  const split = screen.getByRole('listitem', { name: 'Split' });
  expect(within(split).getByText('Versions differ')).toBeVisible();
  fireEvent.click(within(split).getByRole('button', { name: /^Sync/ }));
  const chooser = await screen.findByRole('dialog', {
    name: 'Sync from which device?',
  });
  fireEvent.click(within(chooser).getByRole('button', { name: /Tablet/ }));
  expect(await within(split).findByRole('alert')).toHaveTextContent(
    "This device's copy changed meanwhile",
  );
  expect(
    calls.filter((c) => c.method === 'POST').map((c) => [c.path, c.body]),
  ).toEqual([
    [
      'relay/devices/tablet/api/management/connections/dup/transfer-authorizations',
      { targetDeviceId: 'desk', revision: 'r3' },
    ],
    [
      'api/management/connections/dup/sync',
      {
        sourceDeviceId: 'tablet',
        revision: 'r3',
        token: 't',
        expectedRevision: 'r1',
      },
    ],
  ]);
});

it('keeps unsaved edits through close, save failure and an edit conflict until the user reloads', async () => {
  let version = 1;
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        local({
          name: version === 1 ? 'Work API' : 'Work (server)',
          revision: `r${version}`,
          headers: { 'X-Org': '[redacted]' },
        }),
      ]);
    if (path === 'api/management/connections' && method === 'POST') {
      version = 2;
      throw fail(409, 'edit_conflict');
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  await openEdit('Work API');
  const dialog = await screen.findByRole('dialog', { name: 'Edit connection' });
  const name = within(dialog).getByLabelText('Name');
  expect(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  ).toBeDisabled();
  fireEvent.change(name, { target: { value: 'Work renamed' } });
  fireEvent.keyDown(dialog, { key: 'Escape' });
  expect(
    await screen.findByRole('alertdialog', { name: 'Discard changes?' }),
  ).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Keep editing' }));
  expect(within(dialog).getByLabelText('Name')).toHaveValue('Work renamed');
  // A changed endpoint needs the key again before saving.
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://other.test/v1' },
  });
  expect(
    within(dialog).getByText(
      'The URL or API format changed, so enter the key again.',
    ),
  ).toBeVisible();
  expect(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  ).toBeDisabled();
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://work.test/v1' },
  });
  // A different wire format cannot reuse the saved key either.
  fireEvent.change(within(dialog).getByLabelText('API format'), {
    target: { value: 'openai_chat' },
  });
  expect(within(dialog).getByLabelText('API key')).toBeRequired();
  expect(
    within(dialog).getByText(
      'The URL or API format changed, so enter the key again.',
    ),
  ).toBeVisible();
  expect(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  ).toBeDisabled();
  fireEvent.change(within(dialog).getByLabelText('API format'), {
    target: { value: 'openai_responses' },
  });
  expect(within(dialog).getByLabelText('API key')).not.toBeRequired();
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  );
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'Someone else changed this connection',
  );
  expect(within(dialog).getByLabelText('Name')).toHaveValue('Work renamed');
  expect(calls.find((c) => c.method === 'POST')?.body).toEqual({
    id: 'work',
    revision: 'r1',
    name: 'Work renamed',
    protocol: 'openai_responses',
    baseUrl: 'https://work.test/v1',
    model: 'model-a',
  });
  fireEvent.click(
    within(dialog).getByRole('button', { name: /Load the latest version/ }),
  );
  await waitFor(() =>
    expect(within(dialog).getByLabelText('Name')).toHaveValue('Work (server)'),
  );
  expect(within(dialog).queryByRole('alert')).toBeNull();
});

it('lets Content Quality reuse the form and picker without losing its unsaved writing preferences', async () => {
  let created = false;
  server(({ path, method, body }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        local({
          id: 'chat',
          name: 'Chat One',
          protocol: 'openai_chat',
          model: 'c1',
        }),
        local({ id: 'resp', name: 'Responses Only' }),
        {
          id: 'far',
          name: 'Far Chat',
          protocol: 'openai_chat',
          revision: 'r1',
          local: false,
          sources: [
            {
              deviceId: 'laptop',
              name: 'Laptop',
              online: true,
              revision: 'r1',
            },
          ],
        },
        ...(created
          ? [
              local({
                id: 'new',
                name: 'Plugin Conn',
                protocol: 'openai_chat',
                model: 'p1',
              }),
            ]
          : []),
      ]);
    if (path === 'api/management/connections' && method === 'POST') {
      created = true;
      return local({ ...body, id: 'new', revision: 'r1' });
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  const quality = builtinFrontendPlugins.find(
    (p) => p.manifest.id === CONTENT_QUALITY_PLUGIN_ID,
  )!;
  const save = vi.fn(async (snapshot) => ({ ...snapshot, revision: 2 }));
  render(
    <>
      {quality.settingsSections![0]!.render({
        pluginId: CONTENT_QUALITY_PLUGIN_ID,
        load: async () => ({
          revision: 1,
          values: { connectionFile: '/legacy.json', readerBackground: '' },
        }),
        save,
        connections: {
          renderPicker: (props: UpstreamConnectionPickerProps) => (
            <ConnectionPicker apiRoot="api" {...props} />
          ),
        },
      })}
    </>,
  );
  const picker = await screen.findByLabelText('Saved connection');
  // Only the saved-connection picker: no profile file path, old value kept.
  expect(screen.queryByText(/Connection profile file/)).toBeNull();
  expect(
    screen.getByLabelText('Auto-polish trigger length (characters)'),
  ).toBeVisible();
  await waitFor(() =>
    expect(
      within(picker).getByRole('option', { name: 'Chat One' }),
    ).toBeEnabled(),
  );
  expect(
    within(picker).getByRole('option', { name: 'Responses Only' }),
  ).toBeEnabled();
  expect(within(picker).queryByRole('option', { name: /Far Chat/ })).toBeNull();
  expect(screen.getByText('Far Chat')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Sync' })).toBeEnabled();
  fireEvent.change(screen.getByLabelText('Reader background'), {
    target: { value: 'Keep me' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add connection' }));
  const dialog = await screen.findByRole('dialog', { name: 'Add connection' });
  expect(
    within(within(dialog).getByLabelText('API format'))
      .getAllByRole('option')
      .filter((o) => !(o as HTMLOptionElement).disabled),
  ).toHaveLength(4);
  fireEvent.change(within(dialog).getByLabelText('API format'), {
    target: { value: 'openai_chat' },
  });
  fireEvent.change(within(dialog).getByLabelText('Name'), {
    target: { value: 'Plugin Conn' },
  });
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://plugin.test/v1' },
  });
  fireEvent.change(within(dialog).getByLabelText('API key'), {
    target: { value: 'k' },
  });
  fireEvent.change(within(dialog).getByLabelText('Default model'), {
    target: { value: 'p1' },
  });
  await act(async () => {
    fireEvent.submit(
      within(dialog).getByRole('button', { name: 'Save connection' }),
    );
  });
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  // The connection form is portalled inside the plugin form; it must not submit it.
  expect(save).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Reader background')).toHaveValue('Keep me');
  await waitFor(() =>
    expect(screen.getByLabelText('Saved connection')).toHaveValue('new'),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]![0].values).toMatchObject({
    connectionId: 'new',
    model: 'p1',
    connectionFile: '/legacy.json',
    readerBackground: 'Keep me',
  });
  expect(calls.some((c) => c.path.includes('/apply'))).toBe(false);
});

it('limits the shared form and picker to the protocols a caller supports', async () => {
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        local({ id: 'chat', name: 'Chat One', protocol: 'openai_chat' }),
        local({ id: 'ant', name: 'Anthropic One', protocol: 'anthropic' }),
      ]);
    throw Error(`unexpected ${method} ${path}`);
  });
  const onChange = vi.fn();
  render(
    <ConnectionPicker
      apiRoot="api"
      connectionId=""
      model=""
      protocols={['anthropic']}
      onChange={onChange}
    />,
  );
  const picker = await screen.findByLabelText('Saved connection');
  await waitFor(() =>
    expect(
      within(picker).getByRole('option', { name: 'Anthropic One' }),
    ).toBeEnabled(),
  );
  expect(
    within(picker).getByRole('option', { name: /Chat One/ }),
  ).toBeDisabled();
  fireEvent.change(picker, { target: { value: 'ant' } });
  expect(onChange).toHaveBeenCalledWith({
    connectionId: 'ant',
    model: 'model-a',
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add connection' }));
  const dialog = await screen.findByRole('dialog', { name: 'Add connection' });
  expect(
    within(within(dialog).getByLabelText('API format'))
      .getAllByRole('option')
      .map((o) => o.textContent),
  ).toEqual(['Anthropic Messages']);
});

it('keeps Reapply on a bound connection and flags saved or synced changes until they are applied', async () => {
  let revision = 'r1';
  let applied = { revision: 'r1', model: 'model-b' };
  server(({ path, method, body }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog(
        [
          local({
            revision,
            sources: [
              {
                deviceId: 'laptop',
                name: 'Laptop',
                online: true,
                revision: 'r9',
              },
            ],
          }),
        ],
        {
          bindings: {
            codex: { connectionId: 'work', profileId: 'p', ...applied },
          },
        },
      );
    if (path === 'api/management/connections' && method === 'POST') {
      revision = 'r2';
      return local({ ...body, revision });
    }
    if (path.endsWith('/transfer-authorizations')) return { token: 't' };
    if (path === 'api/management/connections/work/sync') {
      revision = 'r9';
      return local({ revision });
    }
    if (path === 'api/management/connections/work/models')
      return { models: [] };
    if (path === 'api/management/connections/work/apply') {
      applied = { revision, model: String(body.model) };
      return { ok: true };
    }
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  let row = await screen.findByRole('listitem', { name: 'Work API' });
  const codexRow = await harnessRow('Codex');
  // r1 is applied, but Laptop holds r9: the devices differ, the harness is current.
  expect(within(row).getByText('Versions differ')).toBeVisible();
  await waitFor(() => expect(within(row).getByText('Codex')).toBeVisible());
  expect(within(codexRow).queryByText('Not applied')).toBeNull();
  expect(within(codexRow).getByTestId('harness-current')).toHaveTextContent(
    'Work API · model-b',
  );
  const reapply = () =>
    within(codexRow).getByRole('button', { name: 'Reapply Work API to Codex' });
  expect(reapply()).toHaveTextContent(/^Reapply$/);
  expect(reapply()).toBeEnabled();
  await openEdit('Work API');
  const dialog = await screen.findByRole('dialog', { name: 'Edit connection' });
  fireEvent.change(within(dialog).getByLabelText('Default model'), {
    target: { value: 'model-c' },
  });
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  );
  expect(
    await screen.findByText(
      'Work API saved. Harnesses using it keep the applied version until you reapply.',
    ),
  ).toBeVisible();
  row = screen.getByRole('listitem', { name: 'Work API' });
  await waitFor(() =>
    expect(within(row).getByText('Codex · not applied')).toBeVisible(),
  );
  expect(within(codexRow).getByText('Not applied')).toBeVisible();
  expect(calls.some((c) => c.path.endsWith('/apply'))).toBe(false);

  fireEvent.click(reapply());
  const apply = await screen.findByRole('dialog', {
    name: 'Apply Work API to Codex',
  });
  // The harness keeps its applied model unless the user changes it here.
  expect(within(apply).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.change(within(apply).getByLabelText('Default model'), {
    target: { value: 'model-c' },
  });
  fireEvent.click(within(apply).getByRole('button', { name: 'Apply' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await waitFor(() =>
    expect(within(codexRow).queryByText('Not applied')).toBeNull(),
  );
  expect(
    calls.filter((c) => c.path.endsWith('/apply')).map((c) => c.body),
  ).toEqual([{ harness: 'codex', model: 'model-c' }]);

  // Syncing a newer source version also leaves the harness on the applied one.
  fireEvent.click(
    within(screen.getByRole('listitem', { name: 'Work API' })).getByRole(
      'button',
      { name: 'Sync Work API to this device' },
    ),
  );
  await waitFor(() =>
    expect(within(codexRow).getByText('Not applied')).toBeVisible(),
  );
  expect(calls.find((c) => c.path.endsWith('/work/sync'))?.body).toMatchObject({
    expectedRevision: 'r2',
    revision: 'r9',
  });
  expect(calls.filter((c) => c.path.endsWith('/apply'))).toHaveLength(1);
  expect(reapply()).toBeEnabled();
});

it('makes the picker ask which device to sync from when versions differ, and keeps one click otherwise', async () => {
  let synced: string[] = [];
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([
        {
          id: 'split',
          name: 'Split Chat',
          protocol: 'openai_chat',
          revision: 'r2',
          local: false,
          conflict: true,
          sources: [
            {
              deviceId: 'laptop',
              name: 'Laptop',
              online: true,
              revision: 'r2',
            },
            {
              deviceId: 'tablet',
              name: 'Tablet',
              online: true,
              revision: 'r3',
            },
          ],
        },
        {
          id: 'single',
          name: 'Single Chat',
          protocol: 'openai_chat',
          revision: 'r5',
          local: false,
          sources: [
            {
              deviceId: 'laptop',
              name: 'Laptop',
              online: true,
              revision: 'r5',
            },
            { deviceId: 'nas', name: 'NAS', online: true, revision: 'r5' },
          ],
        },
      ]);
    if (path.endsWith('/transfer-authorizations')) {
      synced.push(path);
      return { token: 't' };
    }
    if (path.endsWith('/sync')) return local({ id: 'x', name: 'Synced' });
    throw Error(`unexpected ${method} ${path}`);
  });
  render(
    <ConnectionPicker
      apiRoot="api"
      connectionId=""
      model=""
      onChange={vi.fn()}
    />,
  );
  const split = (await screen.findByText('Split Chat')).closest('li')!;
  fireEvent.click(
    within(split as HTMLElement).getByRole('button', { name: 'Sync' }),
  );
  const choice = within(split as HTMLElement).getByRole('group', {
    name: 'Sync from which device?',
  });
  expect(synced).toEqual([]);
  fireEvent.click(within(choice).getByRole('button', { name: /Tablet/ }));
  await waitFor(() =>
    expect(synced).toEqual([
      'relay/devices/tablet/api/management/connections/split/transfer-authorizations',
    ]),
  );
  expect(calls.find((c) => c.path.endsWith('/split/sync'))?.body).toMatchObject(
    { sourceDeviceId: 'tablet', revision: 'r3' },
  );
  synced = [];
  const single = screen.getByText('Single Chat').closest('li')!;
  fireEvent.click(
    within(single as HTMLElement).getByRole('button', { name: 'Sync' }),
  );
  await waitFor(() => expect(synced).toHaveLength(1));
  expect(
    within(single as HTMLElement).queryByRole('group', {
      name: 'Sync from which device?',
    }),
  ).toBeNull();
});

it('tells users of an older Supervisor to update instead of offering a dead Add button', async () => {
  server(() => {
    throw fail(404, undefined, 'Not Found');
  });
  renderPage();
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Update this Supervisor to manage shared connections.',
  );
  expect(screen.getByRole('button', { name: 'Add connection' })).toBeDisabled();
  cleanup();
  render(
    <ConnectionPicker
      apiRoot="api"
      connectionId=""
      model=""
      onChange={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent(
    'Update this Supervisor to manage shared connections.',
  );
  expect(screen.getByRole('button', { name: 'Add connection' })).toBeDisabled();
});

it('opens the row menu as a keyboard menu and confirms removal with Cancel focused', async () => {
  server(({ path, method }) => {
    if (path === 'api/management/connections' && method === 'GET')
      return catalog([local()]);
    throw Error(`unexpected ${method} ${path}`);
  });
  renderPage();
  const trigger = await screen.findByRole('button', {
    name: 'More actions for Work API',
  });
  fireEvent.click(trigger);
  const menu = screen.getByRole('menu', { name: 'More actions for Work API' });
  const items = within(menu).getAllByRole('menuitem');
  expect(items.map((item) => item.textContent)).toEqual([
    'Edit',
    'Test connection',
    'Remove from this device',
  ]);
  expect(items[0]).toHaveFocus();
  fireEvent.keyDown(menu, { key: 'ArrowUp' });
  expect(items[2]).toHaveFocus();
  fireEvent.keyDown(menu, { key: 'Home' });
  expect(items[0]).toHaveFocus();
  fireEvent.keyDown(menu, { key: 'ArrowDown' });
  expect(items[1]).toHaveFocus();
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('menu')).toBeNull();
  expect(trigger).toHaveFocus();
  fireEvent.click(trigger);
  fireEvent.click(
    screen.getByRole('menuitem', { name: 'Remove from this device' }),
  );
  const confirm = await screen.findByRole('dialog', {
    name: 'Remove connection from this device?',
  });
  expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus();
});
