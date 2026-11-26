import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import { fetchAgentIsolation } from '../../lib/api';
import { DeviceCard, type DeviceCardProps } from './DeviceCard';

vi.mock('../../components/DeviceMonitor', () => ({ DeviceMonitor: () => null }));
vi.mock('../../lib/api', () => ({ fetchAgentIsolation: vi.fn() }));

const today = { range: 'today', generatedAt: '', totals: { turns: 0, completed: 0, failed: 0, interrupted: 0, avgDurationMs: 0, p50DurationMs: 0, inputTokens: 0, outputTokens: 0, cachedTokens: 0, costUsd: 0 }, daily: [], byModel: [], byWorkspace: [], longestThreads: [], activeThreads: 0, runningTurns: 0 } as DeviceCardProps['today'];
const card = (connected: boolean, isolationRevision = 0): DeviceCardProps => ({ device: { id: 'd', name: 'mac', connected, lastHeartbeatAt: null }, metric: undefined, today, history: [], counts: { running: 0, attention: 0, workspaces: 0, threads: 0 }, cacheLabel: '', partial: false, loading: false, skeleton: false, onUpdate: vi.fn(), menuItems: [], isolationRevision });

it('marks an isolated online device with a lock and the word, and reads it again after its settings close', async () => {
  setLocale('en', false);
  vi.mocked(fetchAgentIsolation).mockResolvedValue({ device: true, workspaces: [] });
  const view = render(<MemoryRouter><DeviceCard {...card(true)} /></MemoryRouter>);
  const mark = await screen.findByRole('button', { name: 'Isolated' });
  expect(mark).toBeVisible();
  // What isolation means is a tap away, not a hover title (§8).
  expect(mark).not.toHaveAttribute('title');
  fireEvent.click(mark);
  expect(screen.getByRole('dialog', { name: 'Isolated' })).toHaveTextContent(/Agents on other devices cannot list, read or message/);
  fireEvent.keyDown(window, { key: 'Escape' });
  vi.mocked(fetchAgentIsolation).mockResolvedValue({ device: false, workspaces: [] });
  view.rerender(<MemoryRouter><DeviceCard {...card(true, 1)} /></MemoryRouter>);
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Isolated' })).toBeNull());
  expect(fetchAgentIsolation).toHaveBeenCalledTimes(2);
});

it('reads nothing from an offline device', () => {
  vi.mocked(fetchAgentIsolation).mockClear();
  render(<MemoryRouter><DeviceCard {...card(false)} /></MemoryRouter>);
  expect(fetchAgentIsolation).not.toHaveBeenCalled();
});
