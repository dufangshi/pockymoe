import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it, vi } from 'vitest';
import { UsagePanel } from './UsagePanel';
import { partialSummary } from './model';
import type { DesktopIndex } from '../data/types';
const index = { accountId: 'a', devices: [], workspaces: [], threads: [], activity: {}, extras: {}, pulseKeys: [], hydrated: true, refreshing: false, error: null } as unknown as DesktopIndex;
it('dismisses a focused model tooltip when a new range no longer contains that model', () => {
  const summary = partialSummary(index, 'a', 'today', 'UTC');
  summary.byModel = [{ model: 'Model A', turns: 2, tokens: 1200000, costUsd: 3070 }];
  const props = { sources: [{ deviceId: 'a', name: 'Device A', summary }], index, range: 'today' as const, metric: 'tokens' as const, onMetric: () => {}, origin: '' as const };
  const view = render(<MemoryRouter><UsagePanel {...props} /></MemoryRouter>);
  fireEvent.focus(screen.getByRole('group', { name: 'Model distribution' }).querySelector('circle[tabindex]')!);
  expect(screen.getByRole('tooltip')).toHaveTextContent('1,200,000');
  view.rerender(<MemoryRouter><UsagePanel {...props} sources={[]} /></MemoryRouter>);
  expect(screen.queryByRole('tooltip')).toBeNull();
  expect(screen.getByRole('region', { name: 'Account usage' })).toBeVisible();
});
it('shows unavailable tokens instead of zero for legacy activity with price but no token evidence', () => {
  const sources = [{ deviceId: 'old', name: 'Legacy', activity: { since: '2026-11-01', hours: 24, threads: [{ threadId: 't', turns: 17, activeMs: 0, totalMs: 100, priceEstimate: { currency: 'USD' as const, totalUsd: 207.58, inputUsd: 100, outputUsd: 107.58, cachedInputUsd: 0, cacheWriteInputUsd: 0 } }] } }];
  render(<MemoryRouter><UsagePanel sources={sources} index={index} range="all" metric="tokens" onMetric={() => {}} origin="" /></MemoryRouter>);
  expect(document.querySelector('.pd-usage-table tfoot td:nth-child(3)')).toHaveTextContent('—');
  expect(document.querySelector('.pd-usage-table tfoot')).toHaveTextContent('≥ $207.58');
  expect(document.querySelector('.pd-donut-value')).toHaveTextContent('—');
});
it('keeps an unpriced model share unavailable rather than presenting it as zero cost', () => {
  const summary = partialSummary(index, 'a', 'today', 'UTC');
  summary.totals.costUsd = 1; summary.totals.turns = 2; summary.totals.inputTokens = 200; summary.totals.unpricedTokens = 100;
  summary.byModel = [{ model: 'Priced', turns: 1, tokens: 100, costUsd: 1 }, { model: 'Unpriced', turns: 1, tokens: 100, costUsd: null }];
  render(<MemoryRouter><UsagePanel sources={[{ deviceId: 'a', name: 'A', summary }]} index={index} range="today" metric="cost" onMetric={() => {}} origin="" /></MemoryRouter>);
  expect(screen.getByRole('rowheader', { name: 'Unpriced' }).closest('tr')!.lastElementChild).toHaveTextContent('—');
});
function usageSource(id: string, tokens: number) {
  const summary = partialSummary(index, id, 'today', 'UTC');
  summary.totals.turns = tokens ? 1 : 0; summary.totals.inputTokens = tokens; summary.totals.tokens = tokens; summary.totals.costUsd = tokens / 1000;
  return { deviceId: id, name: `Device ${id}`, summary };
}
const panel = (sources: Parameters<typeof UsagePanel>[0]['sources'], extra = {}) => <MemoryRouter><UsagePanel sources={sources} index={index} range="today" metric="tokens" onMetric={() => {}} origin="" {...extra} /></MemoryRouter>;
it('lists only devices with usage, highest first, one page at a time', () => {
  const sources = [...Array.from({ length: 10 }, (_, i) => usageSource(String(i), (i + 1) * 1000)), usageSource('idle', 0)];
  render(panel(sources));
  const names = () => Array.from(document.querySelectorAll('.pd-usage-label')).map(row => row.textContent);
  expect(names()).toEqual(['Device 9', 'Device 8', 'Device 7', 'Device 6', 'Device 5', 'Device 4', 'Device 3', 'Device 2']);
  expect(names()).not.toContain('Device idle');
  fireEvent.click(screen.getByRole('button', { name: 'Next page' }));
  expect(names()).toEqual(['Device 1', 'Device 0']);
  expect(screen.getByRole('button', { name: 'Next page' })).toBeDisabled();
});
it('explains devices without statistics instead of showing an empty table', () => {
  const onShowAll = vi.fn();
  render(panel([{ deviceId: 'old', name: 'Old' }, { deviceId: 'older', name: 'Older' }], { onShowAll }));
  expect(document.querySelector('.pd-table-empty')).toHaveTextContent('too old to count usage by time');
  expect(document.querySelector('.pd-usage-missing')).toHaveTextContent('2 devices have no statistics');
  expect(document.querySelectorAll('.pd-usage-device')).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Show all time' }));
  expect(onShowAll).toHaveBeenCalled();
});
it('expands device models before workspaces and explains missing activity model history', () => {
  const source = usageSource('a', 1000);
  source.summary.byModel = [{ model: '["remote-codex","grok-4.7"]', turns: 2, tokens: 1000, costUsd: null }];
  const view = render(panel([source]));
  fireEvent.click(screen.getByRole('button', { name: 'Device a · 1,000 Tokens' }));
  const details = document.querySelector('.pd-device-details')!;
  expect(details.querySelectorAll('h4')).toHaveLength(1);
  expect(details.querySelector('h4')).toHaveTextContent('Workspace');
  expect(details.querySelector('.pd-device-models thead th')).toHaveTextContent('Model');
  expect(details.querySelector('.pd-device-models')).toHaveTextContent('grok-4.7');
  expect(details.querySelector('.pd-device-models')).toHaveTextContent('—');
  view.unmount();
  const activity = { since: '2026-11-01', hours: 24, threads: [{ threadId: 't', turns: 1, activeMs: 0, totalMs: 0, tokenUsage: { totalTokens: 1000, inputTokens: 1000, outputTokens: 0, cachedInputTokens: 0, reasoningOutputTokens: 0 } }] };
  render(panel([{ deviceId: 'old', name: 'Old', activity }], { range: 'all' }));
  fireEvent.click(screen.getByRole('button', { name: 'Old · 1,000 Tokens' }));
  expect(screen.getByText('This device has no model breakdown')).toBeVisible();
  expect(document.querySelector('.pd-device-models')).toBeNull();
});
it('makes a full long model name readable from its Info mark', () => {
  const source = usageSource('a', 1000);
  const name = 'provider-a-very-long-model-name-with-specialized-reasoning-2026-11-02';
  source.summary.byModel = [{ model: name, turns: 1, tokens: 1000, costUsd: 1 }];
  render(panel([source]));
  fireEvent.click(screen.getByRole('button', { name: 'Full model name' }));
  expect(screen.getByRole('dialog', { name: 'Full model name' })).toHaveTextContent(name);
});
