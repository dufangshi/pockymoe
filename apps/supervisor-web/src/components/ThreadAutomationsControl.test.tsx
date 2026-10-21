import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { PluginProvider } from '@pockymoe/thread-ui';
import { builtinFrontendPlugins } from '@pockymoe/thread-ui/builtin-plugins';
import type { AutomationDto, ThreadDto } from '@pockymoe/shared';
import { request } from '../lib/api';
import { automationTotals } from './ThreadAutomationsControl';
import { ThreadWatchesControl, type NativeWatch } from './ThreadWatchesControl';

vi.mock('../lib/api', () => ({ request: vi.fn() }));
const thread = { id: 'target', provider: 'claude' } as ThreadDto;
const tokens = {
  totalTokens: 12345,
  inputTokens: 12000,
  cachedInputTokens: 1000,
  cacheWriteInputTokens: 200,
  outputTokens: 345,
  reasoningOutputTokens: 45,
};
const price = {
  totalUsd: 1.25,
  inputUsd: 1,
  cachedInputUsd: 0.05,
  cacheWriteInputUsd: 0.05,
  outputUsd: 0.15,
};
function rule(overrides: Partial<AutomationDto> = {}): AutomationDto {
  return {
    id: 'hourly',
    threadId: 'target',
    sourceKind: 'supervisor',
    definition: {
      name: 'Hourly prompt',
      trigger: { kind: 'interval', everySeconds: 3600 },
      action: { kind: 'prompt', text: 'Read inbox' },
    },
    state: 'enabled',
    nextRunAt: '2030-01-01T01:00:00Z',
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    pendingCount: 1,
    missedCount: 4,
    error: null,
    statistics: {
      triggerCount: 505,
      runCount: 501,
      executedActionCount: 500,
      runningActionCount: 1,
      promptTurnCount: 500,
      ambiguousTurnCount: 2,
      missingTurnCount: 1,
      unattributedRunCount: 1,
      usageTurnCount: 497,
      pricedTurnCount: 490,
      tokenUsage: tokens,
      priceEstimate: price,
    },
    ...overrides,
  };
}
const native = {
  id: 'native',
  cron: '*/20 * * * *',
  schedule: 'Every 20 minutes',
  prompt: 'Native prompt',
  recurring: true,
  createdAt: '2026-01-01T00:00:00Z',
  lastTriggeredAt: null,
  expiresAt: null,
  status: 'active',
  triggerCount: 7,
  usageTriggerCount: 7,
  pricedTriggerCount: 7,
  tokenUsage: tokens,
  priceEstimate: price,
} satisfies NativeWatch;
function mockSnapshot(items = [rule()], watches: NativeWatch[] = [native]) {
  vi.mocked(request).mockImplementation(async (url) =>
    String(url).endsWith('/watches')
      ? { watches }
      : String(url).endsWith('/runs')
        ? { runs: [] }
        : { automations: items },
  );
}
beforeEach(() => {
  vi.clearAllMocks();
  setLocale('en', false);
});

it('consolidates native watches and lifetime supervisor statistics into one read-only panel', async () => {
  mockSnapshot();
  render(<ThreadWatchesControl thread={thread} />);
  expect(screen.getAllByRole('button')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog', { name: 'Automation' });
  await within(dialog).findByText('Hourly prompt');
  expect(dialog).toHaveTextContent('Every hour');
  expect(within(dialog).getByText('Every 20 minutes')).toBeVisible();
  const totals = within(dialog).getByRole('region', {
    name: 'Lifetime totals',
  });
  expect(
    within(totals).getByRole('group', { name: 'Triggers' }),
  ).toHaveTextContent('512');
  expect(
    within(totals)
      .getByRole('group', { name: 'Tokens' })
      .querySelector('strong'),
  ).toHaveAttribute('title', '24,690');
  expect(totals).toHaveTextContent('$2.5');
  fireEvent.click(within(dialog).getByText('Details', { exact: true }));
  expect(dialog).toHaveTextContent(
    'Tokens available for 497 of 500 associated turns; USD priced for 490 of 500.',
  );
  expect(dialog).toHaveTextContent(
    'Excluded: 2 ambiguous turns, 1 missing turns; 1 started runs have no turn attribution.',
  );
  expect(within(dialog).queryByRole('textbox')).not.toBeInTheDocument();
  expect(within(dialog).queryByRole('combobox')).not.toBeInTheDocument();
  expect(
    within(dialog).queryByRole('button', {
      name: /create|preview|register|pause|resume|cancel|edit/i,
    }),
  ).not.toBeInTheDocument();
  // Lifetime totals arrive with the list, independently of the 100-row history page.
  expect(
    vi
      .mocked(request)
      .mock.calls.some(([url]) => String(url).endsWith('/runs')),
  ).toBe(false);
  fireEvent.click(
    within(dialog).getByRole('button', { name: 'Execution history' }),
  );
  await within(dialog).findByText('No executions yet.');
  expect(dialog).toHaveTextContent(
    'Latest 100 execution records. Totals above include all historical runs.',
  );
  expect(
    within(totals).getByRole('group', { name: 'Triggers' }),
  ).toHaveTextContent('512');
  expect(
    vi
      .mocked(request)
      .mock.calls.every(([, init]) => !init?.method || init.method === 'GET'),
  ).toBe(true);
});

it('shows inactive history and distinguishes free inbox delivery from unmeasured script models in Chinese', async () => {
  setLocale('zh-CN', false);
  const script = rule({
    id: 'script',
    state: 'cancelled',
    definition: {
      name: 'Script history',
      trigger: { kind: 'at', at: '2026-01-01T00:00:00Z' },
      action: { kind: 'runScript', argv: ['true'], cwd: '.' },
    },
  });
  const inbox = rule({
    id: 'inbox',
    definition: {
      name: 'Inbox delivery',
      trigger: { kind: 'interval', everySeconds: 60 },
      action: { kind: 'notifyInbox', subject: 'Ready', text: 'Result' },
    },
  });
  mockSnapshot([script, inbox], []);
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: '自动化' }));
  const dialog = await screen.findByRole('dialog', { name: '自动化' });
  await within(dialog).findByText('Inbox delivery');
  const inboxCard = within(dialog)
    .getByText('Inbox delivery')
    .closest('article')!;
  fireEvent.click(within(inboxCard).getByText('详情', { exact: true }));
  expect(dialog).toHaveTextContent('被动收件箱投递不产生模型 token 费用。');
  fireEvent.click(within(dialog).getByText('历史与非活跃自动化 (1)'));
  expect(within(dialog).getByText('Script history')).toBeVisible();
  fireEvent.click(
    within(
      within(dialog).getByText('Script history').closest('article')!,
    ).getByText('详情', { exact: true }),
  );
  expect(dialog).toHaveTextContent('脚本独立调用模型的消耗不在此统计');
  expect(dialog).not.toHaveTextContent('automation.');
});

it('reports legacy and unavailable native coverage as unknown rather than a complete zero', async () => {
  const legacy = rule();
  delete legacy.statistics;
  const legacyNative: NativeWatch = { ...native };
  delete legacyNative.triggerCount;
  delete legacyNative.tokenUsage;
  delete legacyNative.priceEstimate;
  mockSnapshot([legacy], [legacyNative]);
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog', { name: 'Automation' });
  const card = (await within(dialog).findByText('Hourly prompt')).closest('article')!;
  expect(card).toHaveTextContent('Triggers —');
  expect(card.querySelector('.automation-rule-usage')).toHaveTextContent('Tokens unknown');
  fireEvent.click(within(card).getByRole('button', { name: 'Details' }));
  expect(
    within(card).getByText('Lifetime statistics unavailable on this Supervisor.'),
  ).toBeVisible();
  expect(
    within(dialog).getByRole('region', { name: 'Lifetime totals' }),
  ).toHaveTextContent('Partial');
  expect(
    within(dialog)
      .getByRole('group', { name: 'Tokens' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  expect(
    within(dialog)
      .getByRole('group', { name: 'Est. cost · USD' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  vi.mocked(request).mockImplementation(async (url) => {
    if (String(url).endsWith('/watches')) throw Error('Offline');
    return { automations: [legacy] };
  });
  fireEvent.click(within(dialog).getByRole('button', { name: 'Refresh' }));
  // What was shown stays; the failed Refresh is a mark after its button.
  expect(await within(dialog).findByRole('alert')).toHaveTextContent(
    'Native watch history is unavailable; totals may omit native watches.',
  );
  expect(within(dialog).getByRole('button', { name: 'Why it failed' })).toBeVisible();
  expect(within(dialog).getByText('Hourly prompt')).toBeVisible();
});

it.each(['en', 'zh-CN'])('labels plugin actions and treats their unmetered costs as unknown in %s', async locale => {
  setLocale(locale, false);
  const plugin = rule({
    id: 'quality',
    definition: {
      name: 'Polish reply',
      trigger: { kind: 'interval', everySeconds: 60 },
      action: { kind: 'pluginAction', pluginId: 'pockymoe.content-quality', action: 'rewriteReply', input: {} },
    },
    statistics: { ...rule().statistics!, tokenUsage: { ...tokens, totalTokens: 0 }, priceEstimate: { ...price, totalUsd: 0 } },
  });
  mockSnapshot([plugin], []);
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: locale === 'en' ? 'Automation' : '自动化' }));
  const dialog = await screen.findByRole('dialog');
  const card = (await within(dialog).findByText('Polish reply')).closest('article')!;
  expect(card).toHaveTextContent(locale === 'en' ? 'Plugin action' : '插件动作');
  expect(card).toHaveTextContent(locale === 'en' ? 'Every minute' : '每分钟');
  expect(card).not.toHaveTextContent(locale === 'en' ? 'Every 1 minutes' : '每 1 分钟');
  expect(card.querySelector('.automation-rule-usage')).toHaveTextContent(locale === 'en' ? 'Tokens unknownCost unknown' : 'Token 未知费用未知');
  fireEvent.click(within(card).getByText(locale === 'en' ? 'Details' : '详情', { exact: true }));
  expect(card).toHaveTextContent(locale === 'en' ? 'may incur model charges' : '可能产生模型费用');
  expect(card).not.toHaveTextContent(locale === 'en' ? 'no model-token charge' : '不产生模型 token 费用');
  expect(automationTotals([plugin], [])).toMatchObject({ usage: null, price: null });
  expect(dialog).not.toHaveTextContent('automation.pluginAction');
});

it('discards late snapshots when switching threads', async () => {
  let resolve!: (v: unknown) => void;
  vi.mocked(request).mockImplementation(async (url) =>
    String(url).includes('/target/')
      ? new Promise((r) => {
          resolve = r;
        })
      : { automations: [] },
  );
  const { rerender } = render(
    <ThreadWatchesControl thread={{ ...thread, provider: 'codex' }} />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  await waitFor(() => expect(resolve).toBeDefined());
  rerender(
    <ThreadWatchesControl
      thread={{ ...thread, id: 'second', provider: 'codex' }}
    />,
  );
  await act(async () => {
    resolve({ automations: [rule()] });
  });
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  await screen.findByText('No automations yet');
  expect(screen.queryByText('Hourly prompt')).not.toBeInTheDocument();
});

it('shows automatic document findings and diffs from plugin run receipts, even after disabling the plugin', async () => {
  const quality = rule({ id: 'quality', definition: {
    name: 'Content Quality after turn', trigger: { kind: 'interval', everySeconds: 60 },
    action: { kind: 'pluginAction', pluginId: 'pockymoe.content-quality', action: 'afterTurn', input: {} },
  } });
  const report = { status: 'completed', reply: { status: 'skipped', reason: 'autoRepliesDisabled' }, documents: [
    { path: 'guide.md', status: 'reviewed', written: false, findings: ['Explain the retry limit before the example.'], diff: '-retry forever\n+retry up to three times', diffTruncated: true },
    { path: 'notes.md', status: 'failed', error: 'Document changed during review' },
  ], documentsTruncated: true };
  const run = { id: 'report-run', state: 'completed', missedCount: 0, scheduledAt: '2026-10-15T00:00:00Z', startedAt: null, completedAt: null, deliveryReceipt: { pluginId: 'pockymoe.content-quality', action: 'afterTurn', output: report } };
  vi.mocked(request).mockImplementation(async url => String(url).endsWith('/watches') ? { watches: [] } : String(url).endsWith('/runs') ? { runs: [run] } : { automations: [quality] });
  const plugin = builtinFrontendPlugins.find(plugin => plugin.manifest.id === 'pockymoe.content-quality')!;
  const adapter = { fetchPlugins: () => [{ ...plugin.manifest, enabled: false, available: true }] };
  render(<PluginProvider adapter={adapter} builtinPlugins={builtinFrontendPlugins}><ThreadWatchesControl thread={thread} /></PluginProvider>);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog');
  const card = (await within(dialog).findByText('Content Quality after turn')).closest('article')!;
  fireEvent.click(within(card).getByRole('button', { name: 'Execution history' }));
  const receipt = await within(card).findByLabelText('Plugin report');
  expect(within(receipt).getByRole('heading', { name: 'guide.md' })).toBeVisible();
  expect(within(receipt).getByText('Explain the retry limit before the example.', { exact: true })).toBeVisible();
  expect(within(receipt).getByText('File unchanged')).toBeVisible();
  expect(within(receipt).getByText('Document changed during review', { exact: true })).toBeVisible();
  expect(within(receipt).getByText('Additional eligible documents were not reviewed in this run.')).toBeVisible();
  expect(within(receipt).getByText('1 finding')).toBeVisible();
  expect(within(receipt).getByText('Failed')).toBeVisible();
  // A completed afterTurn report does not repeat the run's own status.
  expect(within(receipt).queryByText('Completed')).not.toBeInTheDocument();
  expect(within(receipt).getByText('Automatic reply polishing is disabled')).toBeVisible();
  const documentReport = within(receipt).getByRole('heading', { name: 'guide.md' }).closest('article')!;
  const diff = documentReport.querySelector('pre')!;
  expect(diff).not.toBeVisible();
  fireEvent.click(within(receipt).getByText('Proposed diff'));
  expect(diff).toBeVisible();
  expect(diff).toHaveTextContent('-retry forever +retry up to three times');
  expect(diff.querySelector('.is-add')).toHaveTextContent('+retry up to three times');
  // Raw JSON stays available behind a disclosure.
  expect(within(receipt).getByText('Raw report (JSON)')).toBeVisible();
  expect(within(receipt).getByText('Output truncated for display.')).toBeVisible();
});

it('shows bounded JSON for an unknown plugin output receipt', async () => {
  const unknown = rule({ id: 'unknown-plugin', definition: { name: 'Unknown plugin', trigger: { kind: 'interval', everySeconds: 60 }, action: { kind: 'pluginAction', pluginId: 'third-party.plugin', action: 'report' } } });
  vi.mocked(request).mockImplementation(async url => String(url).endsWith('/watches') ? { watches: [] } : String(url).endsWith('/runs') ? { runs: [{ id: 'unknown-run', state: 'completed', missedCount: 0, scheduledAt: '2026-10-15T00:00:00Z', startedAt: null, completedAt: null, deliveryReceipt: { output: { result: 'x'.repeat(30000) } } }] } : { automations: [unknown] });
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog');
  const card = (await within(dialog).findByText('Unknown plugin')).closest('article')!;
  fireEvent.click(within(card).getByRole('button', { name: 'Execution history' }));
  const receipt = await within(card).findByLabelText('Plugin report');
  expect(receipt.querySelector('pre')!.textContent!.length).toBe(24000);
  expect(within(receipt).getByText('Output truncated for display.')).toBeVisible();
});

it('keeps unknown usage distinct from zero and sums only supplied attributed amounts', () => {
  const known = rule();
  const unknown = rule({
    id: 'unknown',
    statistics: {
      ...known.statistics!,
      triggerCount: 3,
      tokenUsage: null,
      priceEstimate: null,
    },
  });
  expect(automationTotals([known, unknown], [])).toMatchObject({
    triggers: 508,
    unknownCounts: 0,
    usage: tokens,
    price,
  });
  expect(automationTotals([unknown], [])).toMatchObject({
    triggers: 3,
    usage: null,
    price: null,
  });
  expect(
    automationTotals(
      [],
      [{ ...native, triggerCount: 0, ambiguousTriggerCount: 2 }],
    ),
  ).toMatchObject({ triggers: 0, unknownCounts: 1 });
  expect(automationTotals([], [])).toMatchObject({
    triggers: 0,
    usage: { totalTokens: 0 },
    price: { totalUsd: 0 },
  });
});

it('does not display known zero tokens or USD when both history endpoints fail', async () => {
  vi.mocked(request).mockRejectedValue(new Error('History unavailable'));
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog', { name: 'Automation' });
  await within(dialog).findByRole('alert');
  const totals = within(dialog).getByRole('region', {
    name: 'Lifetime totals',
  });
  expect(
    within(totals)
      .getByRole('group', { name: 'Triggers' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  expect(
    within(totals)
      .getByRole('group', { name: 'Tokens' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  expect(
    within(totals)
      .getByRole('group', { name: 'Est. cost · USD' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  expect(
    within(dialog).queryByText('No recorded automations.'),
  ).not.toBeInTheDocument();
});

it('presents an unsupported endpoint as a quiet device status and keeps raw diagnostics collapsed', async () => {
  vi.mocked(request).mockRejectedValue(
    Object.assign(new Error('Route not found'), { statusCode: 404 }),
  );
  render(<ThreadWatchesControl thread={{ ...thread, provider: 'codex' }} />);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog', { name: 'Automation' });
  await within(dialog).findByText('Update this device to view automations');
  expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByText('Route not found')).not.toBeInTheDocument();
  expect(
    within(dialog)
      .getByRole('group', { name: 'Triggers' })
      .querySelector('strong'),
  ).toHaveTextContent('—');
  fireEvent.click(within(dialog).getByRole('button', { name: 'Why it is unavailable' }));
  expect(screen.getByText('Route not found')).toBeVisible();
});

it('lists runs compactly: newest open, older failures flagged, timestamps and turn behind run details', async () => {
  const runs = [
    { id: 'newest', state: 'completed', missedCount: 0, scheduledAt: '2026-10-15T00:00:00Z', startedAt: '2026-10-15T00:00:00Z', completedAt: '2026-10-15T00:00:42Z', turnId: 'turn-newest', deliveryReceipt: { output: { ok: true } } },
    { id: 'older', state: 'failed', missedCount: 2, scheduledAt: '2026-10-14T00:00:00Z', startedAt: null, completedAt: '2026-10-14T00:00:05Z', turnId: null, error: 'Upstream refused the request' },
  ];
  vi.mocked(request).mockImplementation(async url => String(url).endsWith('/watches') ? { watches: [] } : String(url).endsWith('/runs') ? { runs } : { automations: [rule()] });
  render(<ThreadWatchesControl thread={thread} />);
  fireEvent.click(screen.getByRole('button', { name: 'Automation' }));
  const dialog = await screen.findByRole('dialog');
  const card = (await within(dialog).findByText('Hourly prompt')).closest('article')!;
  fireEvent.click(within(card).getByRole('button', { name: 'Execution history' }));
  const [newest, older] = await within(card).findAllByRole('listitem');
  expect(newest!.querySelector('details')).toHaveAttribute('open');
  expect(within(newest!).getByText('· 42s')).toBeVisible();
  expect(within(newest!).getByLabelText('Plugin report')).toBeVisible();
  expect(within(newest!).getByText('turn-newest')).not.toBeVisible();
  fireEvent.click(within(newest!).getByText('Run details'));
  expect(within(newest!).getByText('turn-newest')).toBeVisible();
  expect(older!.querySelector('details')).not.toHaveAttribute('open');
  expect(within(older!).getByText('Failed')).toBeVisible();
  expect(within(older!).getByText('· 2 merged')).toBeVisible();
  expect(within(older!).getByText('Upstream refused the request', { selector: 'summary span' })).toBeVisible();
  const help = within(card).getByText('Latest 100 execution records. Totals above include all historical runs.');
  expect(help).not.toBeVisible();
  fireEvent.click(within(card).getByLabelText('About execution history'));
  expect(help).toBeVisible();
});
