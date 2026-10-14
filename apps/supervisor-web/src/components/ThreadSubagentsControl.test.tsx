import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import type {
  NativeSubagentDto,
  NativeSubagentDetailDto,
  ThreadDetailDto,
} from '@pockymoe/shared';
import { request } from '../lib/api';
import { ThreadSubagentsControl } from './ThreadSubagentsControl';
vi.mock('../lib/api', () => ({ request: vi.fn() }));
const agent: NativeSubagentDto = {
  id: 'child-1',
  name: 'Review runtime',
  provider: 'codex',
  status: 'running',
  nativeSessionId: 'native-child',
  parentToolCallId: 'spawn-1',
  isBackground: true,
  startedAt: '2026-10-09T05:00:00Z',
  completedAt: null,
  updatedAt: '2026-10-09T05:01:00Z',
  latestActivity: 'cargo test passed',
  model: 'gpt-6.1-sol',
  prompt: 'Review changes and check regressions',
  activityCount: 2,
  detailsAvailable: true,
  tokenUsage: {
    total: {
      totalTokens: 12000,
      inputTokens: 10000,
      cachedInputTokens: 5000,
      outputTokens: 2000,
      reasoningOutputTokens: 500,
    },
    last: {
      totalTokens: 12000,
      inputTokens: 10000,
      cachedInputTokens: 5000,
      outputTokens: 2000,
      reasoningOutputTokens: 500,
    },
    modelContextWindow: null,
  },
  priceEstimate: {
    pricingModelKey: 'gpt-6.1-sol',
    pricingTierKey: 'standard',
    currency: 'USD',
    totalUsd: 0.12,
    inputUsd: 0.01,
    cachedInputUsd: 0.01,
    outputUsd: 0.1,
  },
};
const detail = {
  thread: { id: 'parent', provider: 'codex', status: 'running' },
  activeSubagents: [],
} as unknown as ThreadDetailDto;
let inspected: NativeSubagentDetailDto;
beforeEach(() => {
  vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
  localStorage.clear();
  setLocale('en');
  inspected = {
    agent: { ...agent },
    hasEarlierItems: false,
    items: [
      {
        id: 'tool',
        kind: 'toolCall',
        text: 'exec_command\ncargo test\n\nTests passed',
        status: 'completed',
        createdAt: '2026-10-09T05:00:30Z',
      },
    ],
  };
  vi.mocked(request)
    .mockReset()
    .mockImplementation(async (url) =>
      String(url).endsWith('/subagents') ? { agents: [agent] } : inspected,
    );
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it('opens native Codex details with tool progress, timestamp and its own usage', async () => {
  render(<ThreadSubagentsControl detail={detail} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Subagents · 1 running' }));
  const panel = screen.getByRole('dialog', { name: 'Native subagents' });
  fireEvent.click(
    within(panel).getByRole('button', { name: /Review runtime/ }),
  );
  await waitFor(() =>
    expect(within(panel).getByText(/exec_command\s+cargo test/)).toBeVisible(),
  );
  expect(within(panel).getByText('Tokens & estimated cost')).toBeVisible();
  expect(within(panel).getByText('Last update')).toBeVisible();
  expect(
    within(panel).getByText('Review changes and check regressions'),
  ).toBeVisible();
  expect(request).toHaveBeenCalledWith(
    'api/threads/parent/subagents/child-1',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  fireEvent.click(
    within(panel).getByRole('button', { name: 'Back to subagents' }),
  );
  await waitFor(() =>
    expect(document.activeElement).toBe(
      within(panel).getByRole('button', { name: /Review runtime/ }),
    ),
  );
});

it('keeps completed native details open and reports missing costs as unavailable', async () => {
  render(<ThreadSubagentsControl detail={detail} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Subagents · 1 running' }));
  const panel = screen.getByRole('dialog', { name: 'Native subagents' });
  fireEvent.click(
    within(panel).getByRole('button', { name: /Review runtime/ }),
  );
  await waitFor(() =>
    expect(within(panel).getByText('Last update')).toBeVisible(),
  );
  inspected = {
    ...inspected,
    agent: { ...agent, status: 'completed', priceEstimate: null },
  };
  fireEvent.click(within(panel).getByRole('button', { name: 'Refresh' }));
  await waitFor(() =>
    expect(within(panel).getByText('Cost unavailable')).toBeVisible(),
  );
  expect(panel).toBeVisible();
  expect(within(panel).getByText('2 activities')).toBeVisible();
});

it('retains legacy Claude discovery when an older Supervisor lacks native inspection', async () => {
  vi.mocked(request).mockRejectedValue(new Error('Route not found'));
  const legacy = {
    ...detail,
    thread: { ...detail.thread, provider: 'acp', agentId: 'claude' },
    activeSubagents: [{ ...agent, id: 'toolu-1', name: 'Claude review' }],
  } as ThreadDetailDto;
  render(<ThreadSubagentsControl detail={legacy} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Subagents · 1 running' }));
  const panel = screen.getByRole('dialog', { name: 'Native subagents' });
  fireEvent.click(within(panel).getByRole('button', { name: /Claude review/ }));
  await expect(screen.findByRole('alert')).resolves.toHaveTextContent(
    'Route not found',
  );
  expect(within(panel).getByText('Tokens & estimated cost')).toBeVisible();
});

it('drops late native responses when the parent thread changes', async () => {
  let oldResolve!: (value: { agents: NativeSubagentDto[] }) => void;
  vi.mocked(request).mockImplementation(async (url) =>
    String(url).includes('/parent/')
      ? new Promise((resolve) => {
          oldResolve = resolve;
        })
      : { agents: [] },
  );
  const view = render(<ThreadSubagentsControl detail={detail} />);
  view.rerender(
    <ThreadSubagentsControl
      detail={{ ...detail, thread: { ...detail.thread, id: 'another' } }}
    />,
  );
  oldResolve({ agents: [agent] });
  await waitFor(() =>
    expect(screen.queryByRole('button', { name: 'Subagents · 1 running' })).toBeNull(),
  );
  expect(screen.queryByRole('button', { name: 'Subagents · 1 running' })).toBeNull();
});

it('continues history discovery and child updates even when the parent is idle', async () => {
  vi.useFakeTimers();
  let calls = 0;
  vi.mocked(request).mockImplementation(async () => {
    calls++;
    return calls === 1
      ? { agents: [], refreshing: true }
      : {
          agents: [{ ...agent, status: calls === 2 ? 'running' : 'completed' }],
        };
  });
  try {
    await act(async () => {
      render(
        <ThreadSubagentsControl
          detail={{ ...detail, thread: { ...detail.thread, status: 'idle' } }}
        />,
      );
    });
    expect(calls).toBe(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByRole('button', { name: 'Subagents · 1 running' })).toBeVisible();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByRole('button', { name: 'Native subagents' })).toBeVisible();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(9000);
    });
    expect(calls).toBe(3);
  } finally {
    cleanup();
    vi.useRealTimers();
  }
});

it.each([
  { provider: 'claude' as const, agentId: null },
  { provider: 'acp' as const, agentId: 'claude' },
])(
  'discovers Claude native history through $provider without a live runtime fallback',
  async (backend) => {
    const claude = {
      ...agent,
      provider: 'claude',
      name: 'Claude native review',
      status: 'completed',
    };
    vi.mocked(request).mockImplementation(async (url) =>
      String(url).endsWith('/subagents')
        ? { agents: [claude] }
        : { ...inspected, agent: claude },
    );
    render(
      <ThreadSubagentsControl
        detail={{
          ...detail,
          thread: { ...detail.thread, ...backend, status: 'idle' },
        }}
      />,
    );
    fireEvent.click(
      await screen.findByRole('button', { name: 'Native subagents' }),
    );
    const panel = screen.getByRole('dialog', { name: 'Native subagents' });
    fireEvent.click(
      within(panel).getByRole('button', { name: /Claude native review/ }),
    );
    await waitFor(() =>
      expect(
        within(panel).getByText(/exec_command\s+cargo test/),
      ).toBeVisible(),
    );
    expect(within(panel).getByText('Last update')).toBeVisible();
  },
);

it('keeps record bodies lazy through grouped operations and loads older metadata on demand', async () => {
  const rows = [
    { id: 'command-a', kind: 'toolCall' as const, text: 'Run tests', sequence: 31, status: 'completed' },
    { id: 'read-b', kind: 'toolCall' as const, text: 'Read source', sequence: 32, status: 'completed' },
  ];
  const bodyCalls: string[] = [];
  vi.mocked(request).mockImplementation(async url => {
    const parsed = new URL(String(url), 'http://fixture');
    if (parsed.pathname.endsWith('/subagents')) return { agents: [agent] };
    const item = parsed.searchParams.get('itemId');
    if (item) { bodyCalls.push(item); return { ...rows[0], text: 'Full private command output' }; }
    if (parsed.searchParams.has('before')) return { historyMode: 'lazy-v1', agent, items: [{ id: 'older', kind: 'agentMessage', text: 'Earlier checkpoint', sequence: 1 }], hasEarlierItems: false };
    return { historyMode: 'lazy-v1', agent, items: rows, hasEarlierItems: true };
  });
  render(<ThreadSubagentsControl detail={detail} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Subagents · 1 running' }));
  const panel = screen.getByRole('dialog', { name: 'Native subagents' });
  expect(within(panel).getByText(/Created/)).toBeVisible();
  expect(within(panel).getByText(/Updated.*ago/)).toBeVisible();
  expect(vi.mocked(request).mock.calls.every(([url]) => String(url).endsWith('/subagents'))).toBe(true);
  fireEvent.click(within(panel).getByRole('button', { name: /Review runtime/ }));
  const group = await within(panel).findByRole('button', { name: 'Performed 2 operations' });
  expect(bodyCalls).toEqual([]);
  expect(within(panel).queryByText('Run tests')).toBeNull();
  fireEvent.click(group);
  expect(bodyCalls).toEqual([]);
  fireEvent.click(within(panel).getByRole('button', { name: /Run tests/ }));
  await expect(within(panel).findByText('Full private command output')).resolves.toBeVisible();
  expect(bodyCalls).toEqual(['command-a']);
  fireEvent.click(within(panel).getByRole('button', { name: 'Load earlier activity' }));
  await expect(within(panel).findByText('Earlier checkpoint')).resolves.toBeVisible();
  expect(bodyCalls).toEqual(['command-a']);
  expect(within(panel).queryByRole('button', { name: 'Load earlier activity' })).toBeNull();
});

it('keeps unseen completions visible for this visit, then folds them into persistent read history', async () => {
  const completed = { ...agent, status: 'completed', completedAt: '2026-10-09T05:01:30Z' };
  vi.mocked(request).mockImplementation(async () => ({ agents: [completed] }));
  const view = render(<ThreadSubagentsControl detail={detail} />);
  const trigger = await screen.findByRole('button', { name: 'Native subagents' });
  expect(trigger.textContent).toBe('');
  expect(localStorage.getItem('pockymoe.subagents.seen.v1:parent')).toBeNull();
  fireEvent.click(trigger);
  const panel = screen.getByRole('dialog', { name: 'Native subagents' });
  expect(within(panel).getByText('Unread')).toBeVisible();
  expect(panel.querySelector('.native-agents-runtime')).toHaveTextContent('Worked for 1m 30s');
  const cost = panel.querySelector<HTMLButtonElement>('.thread-turn-usage-price')!;
  expect(cost).toHaveTextContent('$0.12');
  fireEvent.click(cost);
  expect(cost).toHaveAttribute('aria-expanded', 'true');
  expect(vi.mocked(request).mock.calls.every(([url]) => String(url).endsWith('/subagents'))).toBe(true);
  expect(within(panel).getByRole('button', { name: /Review runtime/ })).toBeVisible();
  fireEvent.click(within(panel).getByRole('button', { name: 'Close subagents dialog' }));
  fireEvent.click(screen.getByRole('button', { name: 'Native subagents' }));
  expect(screen.queryByRole('button', { name: /Review runtime/ })).toBeNull();
  const history = screen.getByRole('button', { name: 'Read history 1' });
  expect(history).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(history);
  expect(screen.getByRole('button', { name: /Review runtime/ })).toBeVisible();
  view.unmount();
  render(<ThreadSubagentsControl detail={detail} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Native subagents' }));
  expect(screen.queryByRole('button', { name: /Review runtime/ })).toBeNull();
  expect(screen.getByRole('button', { name: 'Read history 1' })).toHaveAttribute('aria-expanded', 'false');
});

it('marks completion unread if a previously seen child runs again, and never mixes parent histories', async () => {
  localStorage.setItem('pockymoe.subagents.seen.v1:parent', JSON.stringify({ 'child-1': 'completed:2026-10-09T05:01:00Z' }));
  vi.mocked(request).mockImplementation(async () => ({ agents: [{ ...agent, status:'failed', completedAt:'2026-10-09T05:02:00Z' }] }));
  const view = render(<ThreadSubagentsControl detail={detail} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Native subagents' }));
  expect(screen.getByText('Unread')).toBeVisible();
  expect(screen.getByRole('button', { name: /Review runtime/ })).toHaveTextContent('Failed');
  view.rerender(<ThreadSubagentsControl detail={{...detail,thread:{...detail.thread,id:'other-parent'}}} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Native subagents' }));
  expect(screen.getByText('Unread')).toBeVisible();
});
