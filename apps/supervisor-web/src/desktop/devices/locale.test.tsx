import { deviceActions } from './deviceActions';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { afterEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { DeviceCard, type DeviceCardProps } from './DeviceCard';
import { HourlyActivityChart } from './Charts';
import { age, compact, duration, money, turns, type StatsSummary } from './model';

vi.mock('../../components/DeviceMonitor', () => ({ DeviceMonitor: () => <button>CPU</button> }));
afterEach(() => setLocale('en', false));
const summary: StatsSummary = { range: 'today', generatedAt: new Date().toISOString(), totals: { turns: 1, completed: 1, failed: 0, interrupted: 0, avgDurationMs: 90000, p50DurationMs: 90000, inputTokens: 20, outputTokens: 10, cachedTokens: 5, costUsd: .01 }, daily: [{ date: '2026-11-01', turns: 1, failed: 0, tokens: 30, costUsd: .01 }], byModel: [], byWorkspace: [], longestThreads: [], activeThreads: 0, runningTurns: 0 };
it('formats durations, relative time, plural counts and numbers in the current shared locale', () => {
  const now = Date.parse('2026-11-01T12:00:00Z');
  setLocale('en', false);
  expect(duration(90000)).toBe('1m 30s'); expect(age('2026-11-01T11:00:00Z', now)).toBe('1 hour ago');
  expect(age('2026-11-01T10:00:00Z', now)).toBe('2 hours ago'); expect(turns(1)).toBe('1 turn'); expect(turns(2)).toBe('2 turns'); expect(compact(1_200_000)).toBe('1.2M');
  setLocale('zh-CN', false);
  expect(duration(90000)).toBe('1 分 30 秒'); expect(age('2026-11-01T10:00:00Z', now)).toBe('2 小时前');
  expect(turns(2)).toBe('2 轮'); expect(compact(1_200_000)).toBe('120万'); expect(money(null)).toBe('—');
});
it('updates open card menus and accessible labels without remounting user/device content', async () => {
  setLocale('en', false);
  const props: DeviceCardProps = { device: { id: 'd', name: '用户设备', connected: true, lastHeartbeatAt: null }, metric: { summaries: { today: summary }, version: '0.13.41', latestVersion: '0.13.42' }, today: summary, history: summary.daily, counts: { running: 0, attention: 0, workspaces: 1, threads: 1 }, cacheLabel: '', partial: false, loading: false, skeleton: false, renameSupported: true, onReveal: vi.fn(), onNewThread: vi.fn(), onRename: vi.fn(), onUpdate: vi.fn(), menuItems: [] };
  function Card() { useI18n(); return <DeviceCard {...props} menuItems={deviceActions(props.device, true, vi.fn(), props.onReveal)} />; }
  render(<MemoryRouter><Card /></MemoryRouter>);
  const card = screen.getByRole('article', { name: '用户设备' });
  expect(screen.getByRole('img', { name: 'No running threads' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Actions for 用户设备' }));
  expect(screen.getByRole('menuitem', { name: 'Show in map' })).toBeVisible();
  act(() => setLocale('zh-CN', false));
  await waitFor(() => expect(screen.getByRole('menuitem', { name: '在地图中显示' })).toBeVisible());
  expect(screen.getByRole('article', { name: '用户设备' })).toBe(card);
  expect(screen.getByRole('img', { name: '没有运行中的线程' })).toBeVisible();
  expect(screen.getByRole('button', { name: '有更新 0.13.42' })).toBeVisible();
  act(() => setLocale('en', false));
  expect(screen.getByRole('menuitem', { name: 'Rename' })).toBeVisible();
});
it('updates the focused chart tooltip and unit while retaining the same data bucket', () => {
  setLocale('en', false);
  render(<HourlyActivityChart sources={[{ deviceId: 'd', name: 'User text', values: Array(24).fill(1) }]} />);
  const chart = screen.getByRole('group', { name: 'Turns by hour today, stacked by device' });
  const bucket = chart.querySelectorAll('g')[0]!;
  fireEvent.focus(bucket);
  expect(screen.getByRole('tooltip')).toHaveTextContent('1 turn');
  act(() => setLocale('zh-CN', false));
  expect(screen.getByRole('group', { name: '今日每小时轮数，按设备堆叠' })).toBe(chart);
  expect(chart.querySelectorAll('g')[0]).toBe(bucket);
  expect(screen.getByRole('tooltip')).toHaveTextContent('1 轮');
});
