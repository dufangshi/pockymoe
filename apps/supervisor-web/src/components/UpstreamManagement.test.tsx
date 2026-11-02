import { changeControl } from '../test/formControls';
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { it, expect, vi, beforeEach } from 'vitest';
import { UpstreamManagement } from './UpstreamManagement';
import { request } from '../lib/api';
vi.mock('../lib/api', () => ({ request: vi.fn() }));
beforeEach(() => vi.mocked(request).mockReset());
it('pins configuration to the selected device and never reuses secrets in duplicated profiles', async () => {
  vi.mocked(request).mockImplementation(async (path) =>
    String(path).endsWith('/models')
      ? { models: [{ id: 'test', name: 'test' }], truncated: false }
      : {
          profiles: [
            {
              id: 'p',
              name: 'Work',
              harness: 'codex',
              baseUrl: 'https://example.test/v1',
              model: 'test',
              apiType: 'responses',
              contextWindow: 100000,
              hasApiKey: true,
            },
          ],
          active: { codex: 'p' },
          backups: [],
        },
  );
  render(<UpstreamManagement apiRoot="/relay/devices/device-a/api" />);
  expect(await screen.findByText('Work')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Use upstream' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Duplicate Work' }));
  expect(screen.getByLabelText('API key')).toHaveValue('');
  expect(screen.getByLabelText('API key')).toBeRequired();
  changeControl(screen.getByLabelText('API key'), {
    target: { value: 'new-secret' },
  });
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Model' })).toBeEnabled());
  fireEvent.click(screen.getByRole('combobox', { name: 'Model' }));
  await screen.findByRole('option', { name: 'test' });
  changeControl(screen.getByLabelText('Model'), {
    target: { value: 'test' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Save upstream' }));
  await waitFor(() =>
    expect(request).toHaveBeenCalledWith(
      '/relay/devices/device-a/api/management/upstreams',
      expect.objectContaining({
        method: 'POST',
        body: expect.stringContaining('new-secret'),
      }),
    ),
  );
});
it('discards stale model discovery when the upstream changes', async () => {
  let resolveOld!: (value: unknown) => void;
  vi.mocked(request).mockImplementation(async (path, options) => {
    if (!String(path).endsWith('/models'))
      return { profiles: [], active: {}, backups: [] };
    const body = JSON.parse(String(options?.body));
    if (body.baseUrl.includes('old'))
      return new Promise((resolve) => {
        resolveOld = resolve;
      });
    return {
      models: [{ id: 'new-model', name: 'new-model' }],
      truncated: false,
    };
  });
  render(<UpstreamManagement apiRoot="/relay/devices/device-b/api" />);
  await screen.findByText(/Add an API provider/);
  fireEvent.click(screen.getByRole('button', { name: 'Add upstream' }));
  changeControl(screen.getByLabelText('Base URL'), {
    target: { value: 'https://old.example/v1' },
  });
  changeControl(screen.getByLabelText('API key'), {
    target: { value: 'key' },
  });
  await waitFor(() => expect(resolveOld).toBeTypeOf('function'));
  changeControl(screen.getByLabelText('Base URL'), {
    target: { value: 'https://new.example/v1' },
  });
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Model' })).toBeEnabled());
  fireEvent.click(screen.getByRole('combobox', { name: 'Model' }));
  await screen.findByRole('option', { name: 'new-model' });
  changeControl(screen.getByLabelText('Model'), {
    target: { value: 'new-model' },
  });
  await act(async () =>
    resolveOld({
      models: [{ id: 'old-model', name: 'old-model' }],
      truncated: false,
    }),
  );
  expect(
    screen.queryByRole('option', { name: 'old-model' }),
  ).not.toBeInTheDocument();
  expect(screen.getByLabelText('Model')).toHaveValue('new-model');
  changeControl(screen.getByLabelText('API key'), {
    target: { value: 'changed-key' },
  });
  expect(screen.getByLabelText('Model')).toHaveValue('');
  expect(screen.getByRole('button', { name: 'Save upstream' })).toBeDisabled();
});
it('previews a template without mutation and applies only on the second action', async () => {
  vi.mocked(request).mockImplementation(async (path, options) =>
    String(path).endsWith('upstreams')
      ? { profiles: [], active: {}, backups: [] }
      : { harnesses: ['codex'], profiles: [] },
  );
  render(<UpstreamManagement apiRoot="/api" templatesOnly />);
  await waitFor(() =>
    expect(
      screen.getByRole('button', { name: 'Import template' }),
    ).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Import template' }));
  fireEvent.click(screen.getByRole('button', { name: 'Preview template' }));
  await screen.findByText('Install if missing: codex');
  expect(
    vi
      .mocked(request)
      .mock.calls.filter(([p]) => String(p).endsWith('templates'))
      .map(([, o]) => JSON.parse(String(o?.body)).apply),
  ).toEqual([false]);
  fireEvent.click(screen.getByRole('button', { name: 'Apply template' }));
  await waitFor(() =>
    expect(
      vi
        .mocked(request)
        .mock.calls.filter(([p]) => String(p).endsWith('templates'))
        .map(([, o]) => JSON.parse(String(o?.body)).apply),
    ).toEqual([false, true]),
  );
});
