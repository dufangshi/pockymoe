import {
  act,
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
import { ApiError, request } from '../lib/api';
import { ConnectionCatalog } from './ConnectionCatalog';
import { ConnectionPicker } from './ConnectionPicker';

vi.mock('../lib/api', () => ({
  request: vi.fn(),
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
    calls.push(call);
    return route(call);
  });
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
const codex = { id: 'codex', name: 'Codex', installed: true };
const claude = { id: 'claude', name: 'Claude Agent', installed: true };

beforeEach(() => {
  vi.mocked(request).mockReset();
});

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
  render(<ConnectionCatalog apiRoot="api" harness={codex} />);
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
  render(<ConnectionCatalog apiRoot="api" harness={codex} />);
  const row = await screen.findByRole('listitem', { name: 'Work API' });
  expect(within(row).queryByRole('button', { name: 'Sync' })).toBeNull();
  expect(
    within(screen.getByRole('listitem', { name: 'Gemini Direct' })).getByRole(
      'button',
      { name: 'Apply to Codex' },
    ),
  ).toBeEnabled();
  expect(screen.getByTestId('harness-binding')).toHaveTextContent(
    'its own sign-in or native configuration',
  );
  fireEvent.click(within(row).getByRole('button', { name: 'Apply to Codex' }));
  const dialog = await screen.findByRole('dialog', {
    name: 'Apply Work API to Codex',
  });
  const choose = await within(dialog).findByLabelText(
    'Choose a discovered model',
  );
  fireEvent.change(choose, { target: { value: 'model-b' } });
  expect(within(dialog).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Apply to Codex' }),
  );
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'The harness is busy',
  );
  expect(within(dialog).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Apply to Codex' }),
  );
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
    expect(
      within(screen.getByRole('listitem', { name: 'Work API' })).getByText(
        'In use',
      ),
    ).toBeVisible(),
  );
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
  render(<ConnectionCatalog apiRoot="api" harness={claude} />);
  const row = await screen.findByRole('listitem', { name: 'Shared Anthropic' });
  expect(within(row).getByText('Other device')).toBeVisible();
  expect(within(row).getByText('Saved on Laptop')).toBeVisible();
  expect(within(row).queryByRole('button', { name: /Apply to/ })).toBeNull();
  expect(within(row).queryByRole('button', { name: /Edit/ })).toBeNull();
  fireEvent.click(within(row).getByRole('button', { name: 'Sync' }));
  expect(await within(row).findByRole('alert')).toHaveTextContent(
    'Device-to-device access is off',
  );
  expect(calls.some((c) => c.path.endsWith('/sync'))).toBe(false);
  fireEvent.click(within(row).getByRole('button', { name: 'Sync' }));
  expect(
    await screen.findByText(
      'Shared Anthropic is now on this device. Apply it to Claude Agent separately when you are ready.',
    ),
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
  const after = screen.getByRole('listitem', { name: 'Shared Anthropic' });
  expect(
    within(after).getByRole('button', { name: 'Apply to Claude Agent' }),
  ).toBeEnabled();
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
  render(<ConnectionCatalog apiRoot="api" harness={codex} />);
  const offline = await screen.findByRole('listitem', { name: 'Offline One' });
  expect(within(offline).getByText('Source offline')).toBeVisible();
  expect(within(offline).getByRole('button', { name: 'Sync' })).toBeDisabled();
  const split = screen.getByRole('listitem', { name: 'Split' });
  expect(within(split).getByText('Versions differ')).toBeVisible();
  fireEvent.click(within(split).getByRole('button', { name: 'Sync' }));
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
  render(<ConnectionCatalog apiRoot="api" />);
  const edit = await screen.findByRole('button', { name: 'Edit Work API' });
  fireEvent.click(edit);
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
    within(dialog).getByText('The URL changed, so enter the key again.'),
  ).toBeVisible();
  expect(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  ).toBeDisabled();
  fireEvent.change(within(dialog).getByLabelText('Base URL'), {
    target: { value: 'https://work.test/v1' },
  });
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
  expect(screen.queryByLabelText('Connection profile file')).toBeNull();
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
  render(<ConnectionCatalog apiRoot="api" harness={codex} />);
  let row = await screen.findByRole('listitem', { name: 'Work API' });
  // r1 is applied, but Laptop holds r9: the row is in conflict, not pending.
  expect(within(row).getByText('In use')).toBeVisible();
  expect(
    within(row).getByRole('button', { name: 'Reapply to Codex' }),
  ).toBeEnabled();
  fireEvent.click(within(row).getByRole('button', { name: 'Edit Work API' }));
  const dialog = await screen.findByRole('dialog', { name: 'Edit connection' });
  fireEvent.change(within(dialog).getByLabelText('Default model'), {
    target: { value: 'model-c' },
  });
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Save connection' }),
  );
  expect(
    await screen.findByText(
      'Work API saved. Codex keeps the applied version until you reapply.',
    ),
  ).toBeVisible();
  row = screen.getByRole('listitem', { name: 'Work API' });
  await waitFor(() =>
    expect(within(row).getByText('Changes not applied')).toBeVisible(),
  );
  expect(screen.getByTestId('harness-binding')).toHaveTextContent(
    'Changes not applied',
  );
  expect(calls.some((c) => c.path.endsWith('/apply'))).toBe(false);

  fireEvent.click(
    within(row).getByRole('button', { name: 'Reapply to Codex' }),
  );
  const apply = await screen.findByRole('dialog', {
    name: 'Apply Work API to Codex',
  });
  // The harness keeps its applied model unless the user changes it here.
  expect(within(apply).getByLabelText('Default model')).toHaveValue('model-b');
  fireEvent.change(within(apply).getByLabelText('Default model'), {
    target: { value: 'model-c' },
  });
  fireEvent.click(
    within(apply).getByRole('button', { name: 'Apply to Codex' }),
  );
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await waitFor(() =>
    expect(
      within(screen.getByRole('listitem', { name: 'Work API' })).getByText(
        'In use',
      ),
    ).toBeVisible(),
  );
  expect(
    calls.filter((c) => c.path.endsWith('/apply')).map((c) => c.body),
  ).toEqual([{ harness: 'codex', model: 'model-c' }]);

  // Syncing a newer source version also leaves the harness on the applied one.
  fireEvent.click(
    within(screen.getByRole('listitem', { name: 'Work API' })).getByRole(
      'button',
      { name: 'Sync' },
    ),
  );
  await waitFor(() =>
    expect(
      within(screen.getByRole('listitem', { name: 'Work API' })).getByText(
        'Changes not applied',
      ),
    ).toBeVisible(),
  );
  expect(calls.find((c) => c.path.endsWith('/work/sync'))?.body).toMatchObject({
    expectedRevision: 'r2',
    revision: 'r9',
  });
  expect(calls.filter((c) => c.path.endsWith('/apply'))).toHaveLength(1);
  expect(
    within(screen.getByRole('listitem', { name: 'Work API' })).getByRole(
      'button',
      { name: 'Reapply to Codex' },
    ),
  ).toBeEnabled();
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
