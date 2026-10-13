import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { HarnessResourceSettings } from './HarnessResourceSettings';
import { parseResourceField } from './harnessResourceModel';
import { ApiError, fetchHarnessResourceSettings, relayModeActive, updateHarnessResourceSettings } from '../lib/api';

vi.mock('../lib/api', () => {
  class ApiError extends Error {
    constructor(public statusCode: number, public payload: { message: string }) {
      super(payload.message);
    }
  }
  return { ApiError, fetchHarnessResourceSettings: vi.fn(), updateHarnessResourceSettings: vi.fn(), relayModeActive: vi.fn() };
});

const GIB = 2 ** 30;
beforeEach(() => {
  vi.mocked(relayModeActive).mockReset().mockReturnValue(false);
  vi.mocked(fetchHarnessResourceSettings).mockReset().mockResolvedValue({ idleReleaseMinutes: 10, maxLoadedThreads: null, maxHarnessMemoryBytes: null });
  vi.mocked(updateHarnessResourceSettings).mockReset().mockImplementation(async input => input);
});
const renderAt = (path = '/threads/t') => render(<MemoryRouter initialEntries={[path]}><HarnessResourceSettings /></MemoryRouter>);
const field = (name: string) => screen.getByRole('spinbutton', { name }) as HTMLInputElement;
const toggle = (name: string) => screen.getByRole('checkbox', { name }) as HTMLInputElement;

it('reads the settings and saves limits in bytes', async () => {
  renderAt();
  await waitFor(() => expect(field('Release idle threads')).toHaveValue(10));
  expect(fetchHarnessResourceSettings).toHaveBeenCalledWith(null);
  expect(toggle('Limit loaded threads')).not.toBeChecked();
  expect(field('Limit loaded threads')).toBeDisabled();
  fireEvent.click(toggle('Limit loaded threads'));
  fireEvent.change(field('Limit loaded threads'), { target: { value: '6' } });
  fireEvent.click(toggle('Limit harness memory'));
  fireEvent.change(field('Limit harness memory'), { target: { value: '2.5' } });
  fireEvent.click(toggle('Release idle threads'));
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(updateHarnessResourceSettings).toHaveBeenCalledWith({
    idleReleaseMinutes: null, maxLoadedThreads: 6, maxHarnessMemoryBytes: 2.5 * GIB,
  }, null));
  expect(await screen.findByRole('status')).toHaveTextContent('Saved.');
});

it('rejects values the Supervisor would reject', async () => {
  renderAt();
  await waitFor(() => expect(field('Release idle threads')).toHaveValue(10));
  const save = screen.getByRole('button', { name: 'Save' });
  fireEvent.change(field('Release idle threads'), { target: { value: '1441' } });
  expect(screen.getByText('Enter 1–1440 minutes.')).toBeVisible();
  expect(save).toBeDisabled();
  fireEvent.change(field('Release idle threads'), { target: { value: '1440' } });
  expect(save).toBeEnabled();
  fireEvent.click(toggle('Limit harness memory'));
  fireEvent.change(field('Limit harness memory'), { target: { value: '0.2' } });
  expect(screen.getByText('Enter at least 0.25 GiB.')).toBeVisible();
  expect(save).toBeDisabled();
  fireEvent.click(save);
  expect(updateHarnessResourceSettings).not.toHaveBeenCalled();
});

it('validates each range like the Supervisor', () => {
  expect(parseResourceField('idleReleaseMinutes', true, '0')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', true, '1.5')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', true, '')).toBeUndefined();
  expect(parseResourceField('idleReleaseMinutes', false, 'x')).toBeNull();
  expect(parseResourceField('maxLoadedThreads', true, '200')).toBe(200);
  expect(parseResourceField('maxLoadedThreads', true, '201')).toBeUndefined();
  expect(parseResourceField('maxHarnessMemoryBytes', true, '0.25')).toBe(256 * 2 ** 20);
  // An untouched limit keeps its exact bytes.
  expect(parseResourceField('maxHarnessMemoryBytes', true, '0.29', 300 * 2 ** 20)).toBe(300 * 2 ** 20);
});

it('targets the routed relay device and asks for one without it', async () => {
  vi.mocked(relayModeActive).mockReturnValue(true);
  const { unmount } = renderAt('/workspaces');
  expect(screen.getByText('Open a device to manage its resources.')).toBeVisible();
  expect(fetchHarnessResourceSettings).not.toHaveBeenCalled();
  unmount();
  renderAt('/devices/dev-1/threads/t');
  await waitFor(() => expect(fetchHarnessResourceSettings).toHaveBeenCalledWith('dev-1'));
});

it('explains an older Supervisor instead of failing', async () => {
  vi.mocked(fetchHarnessResourceSettings).mockRejectedValue(new ApiError(404, { code: 'not_found', message: 'Not found' } as never));
  renderAt();
  expect(await screen.findByText("Update this device's Supervisor to change resource settings.")).toBeVisible();
  expect(screen.queryByRole('alert')).toBeNull();
});
