import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { expect, it } from 'vitest';
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
