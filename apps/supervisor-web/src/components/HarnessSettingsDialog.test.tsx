import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type {
  AgentCapabilitySnapshotDto,
  ModelOptionDto,
  ThreadDto,
} from '@pockymoe/shared';
import {
  HarnessSettingsDialog,
  HarnessSettingsFields,
} from './HarnessSettingsDialog';
import { DshPluginPanel } from './DshHarnessPanel';

const models: ModelOptionDto[] = [
  {
    id: 'model',
    model: 'model',
    displayName: 'GPT-6.1 Sol',
    description: '',
    isDefault: true,
    hidden: false,
    defaultReasoningEffort: 'medium',
    supportedReasoningEfforts: [
      { reasoningEffort: 'medium', description: '' },
      { reasoningEffort: 'high', description: '' },
    ],
  },
];
const thread = {
  id: 'fixture',
  model: 'model',
  reasoningEffort: 'high',
  activeTurnId: null,
  sandboxMode: null,
} as ThreadDto;
const snapshot = {
  negotiated: { harness: { notice: 'Ready', plugins: [] } },
} as unknown as AgentCapabilitySnapshotDto;
const loadCapabilities = vi.fn(async () => snapshot);

describe('session settings fields', () => {
  it('keeps explicit permissions and reasoning when opened and only writes requested settings', async () => {
    const onChange = vi.fn(async () => {});
    render(
      <HarnessSettingsFields
        thread={{ ...thread, sandboxMode: 'read-only' }}
        models={models}
        busy={false}
        onChange={onChange}
        loadCapabilities={loadCapabilities}
      />,
    );
    await screen.findByText('Ready');
    expect(screen.getByRole('combobox', { name: 'Harness model' })).toHaveValue(
      'model',
    );
    expect(
      screen.getByRole('combobox', { name: 'Harness reasoning effort' }),
    ).toHaveValue('high');
    const permissions = screen.getByRole('combobox', {
      name: 'Workspace permissions',
    });
    expect(permissions).toHaveValue('read-only');
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(permissions, { target: { value: 'workspace-write' } });
    expect(onChange).toHaveBeenCalledWith({ sandboxMode: 'workspace-write' });
    fireEvent.change(
      screen.getByRole('combobox', { name: 'Harness reasoning effort' }),
      { target: { value: 'medium' } },
    );
    expect(onChange).toHaveBeenLastCalledWith({ reasoningEffort: 'medium' });
  });

  it('shows full access for unset permissions without changing the thread', async () => {
    const onChange = vi.fn(async () => {});
    render(
      <HarnessSettingsFields
        thread={thread}
        models={models}
        busy={false}
        onChange={onChange}
        loadCapabilities={loadCapabilities}
      />,
    );
    await screen.findByText('Ready');
    expect(
      screen.getByRole('combobox', { name: 'Workspace permissions' }),
    ).toHaveValue('danger-full-access');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('marks the whole portal dialog as owned and keeps running session settings editable', async () => {
    const onOpen = vi.fn();
    const onChange = vi.fn(async () => {});
    render(
      <HarnessSettingsDialog
        thread={{ ...thread, activeTurnId: 'running' }}
        models={models}
        busy={false}
        onChange={onChange}
        onOpen={onOpen}
        liveModelSwitch
        loadCapabilities={loadCapabilities}
        composerFocusOwner="composer-one"
        onClose={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole('dialog')).toHaveAttribute(
        'data-composer-focus-owner',
        'composer-one',
      ),
    );
    // The model list is refreshed where it is about to be read.
    expect(onOpen).toHaveBeenCalledOnce();
    for (const select of screen.getAllByRole('combobox'))
      expect(select).toBeEnabled();
    // The harness takes the change mid-turn and the fields say when.
    expect(screen.getByText(/applies from its next model call/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Harness reasoning effort'), { target: { value: 'medium' } });
    expect(onChange).toHaveBeenCalledWith({ reasoningEffort: 'medium' });
    expect(
      screen
        .getByRole('button', { name: 'Close' })
        .closest('[data-composer-focus-owner]'),
    ).toBe(screen.getByRole('dialog'));
  });
});

describe('session settings timing', () => {
  it('says a change waits for the next turn where the harness cannot take it live', async () => {
    render(
      <HarnessSettingsFields thread={{ ...thread, activeTurnId: 'running' }} models={models} busy={false}
        onChange={vi.fn(async () => {})} loadCapabilities={loadCapabilities} />,
    );
    expect(await screen.findByText(/changes apply from the next turn/)).toBeInTheDocument();
  });
});

describe('DeepSeek Harness panel', () => {
  const dsh = {
    kind: 'dsh',
    version: '0.2.0-rc.2',
    profile: 'acp',
    permissionPresets: ['read-only', 'workspace-write'],
    plugins: [
      { id: 'include:repeat', name: 'Repeat guard', module: '@deepseek-ai/dsh-repeat-tool-reminder', description: '', enabled: true, active: true, readOnly: null },
      { id: 'include:timer', name: 'Timer', module: '@deepseek-ai/cordis-plugin-timer', description: '', enabled: true, active: true, readOnly: 'management-required' },
    ],
    bundles: [{ name: '@deepseek-ai/dsh-base', version: '0.2.0-rc.2', description: '', enabled: true, removable: false, readOnly: 'management-required' }],
    providers: [{ id: 'deepseek-official', name: 'DeepSeek', declared: true }],
    session: {
      running: true,
      projections: {
        permissions: { currentValue: 'read-only' },
        plan: { active: true, pending: false },
        goal: { goal: { objective: 'ship it', phase: 'active' }, roundsStarted: 3 },
        todos: [{ content: 'write tests', status: 'in_progress' }],
      },
    },
  };
  const dshSnapshot = { negotiated: { harness: dsh } } as unknown as AgentCapabilitySnapshotDto;

  it('shows live DSH state and routes typed actions, then restarts to apply them', async () => {
    const applied = { ...dsh, plugins: dsh.plugins.map((plugin) => ({ ...plugin, enabled: plugin.id !== 'include:repeat' })) };
    const runHarnessAction = vi.fn(async (action: { kind: string }) => {
      if (action.kind === 'settings')
        return { settings: [{ ns: 'session-log-deepseek', revision: 4, fields: [{ key: 'enabled', type: 'boolean', value: true, overridden: false }] }] };
      if (action.kind === 'updateSetting')
        return { result: { ns: 'session-log-deepseek', revision: 5, fields: [{ key: 'enabled', type: 'boolean', value: false, overridden: true }] } };
      if (action.kind === 'restart') return { result: { restarted: true } };
      if (action.kind === 'refresh') return { result: null, harness: applied };
      return { result: { application: 'restart-required' }, harness: dsh };
    });
    render(
      <HarnessSettingsFields thread={thread} models={models} busy={false}
        onChange={vi.fn(async () => {})} loadCapabilities={async () => dshSnapshot}
        runHarnessAction={runHarnessAction as never} />,
    );
    await screen.findByText('DeepSeek Harness 0.2.0-rc.2 · acp profile');
    expect(screen.getByText('read-only')).toBeInTheDocument();
    expect(screen.getByText('ship it · active · round 3')).toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Todo list' })).toHaveTextContent('write tests');
    expect(screen.getByRole('checkbox', { name: 'Enable Timer' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'stop' }));

    fireEvent.click(screen.getByRole('checkbox', { name: 'Enable Repeat guard' }));
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'setPluginEnabled', id: 'include:repeat', enabled: false }));
    // The live process still runs the plugin; the saved choice stays visible.
    await waitFor(() => expect(screen.getByRole('checkbox', { name: 'Enable Repeat guard' })).not.toBeChecked());
    expect(screen.getByText('Disabled · applies after reconnect')).toBeInTheDocument();
    fireEvent.click(await screen.findByRole('button', { name: 'Reconnect to apply' }));
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'refresh' }));
    expect(runHarnessAction.mock.calls.map(([action]) => action.kind).slice(-2)).toEqual(['restart', 'refresh']);
    // The restarted process reports the saved choice: nothing is pending now.
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Reconnect to apply' })).toBeNull());
    expect(screen.getByRole('checkbox', { name: 'Enable Repeat guard' })).not.toBeChecked();
    expect(screen.queryByText(/applies after reconnect/)).toBeNull();

    fireEvent.click(screen.getByText('Profile settings'));
    fireEvent(screen.getByText('Profile settings').closest('details')!, new Event('toggle'));
    const toggle = await screen.findByRole('checkbox', { name: 'session-log-deepseek.enabled' });
    fireEvent.click(toggle);
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({
      kind: 'updateSetting', ns: 'session-log-deepseek', key: 'enabled', value: false, revision: 4,
    }));
    await waitFor(() => expect(screen.getByRole('checkbox', { name: 'session-log-deepseek.enabled' })).not.toBeChecked());
  });

  it('keeps every DSH control read-only for viewers', async () => {
    render(
      <HarnessSettingsFields thread={thread} models={models} busy={false} readOnly
        onChange={vi.fn(async () => {})} loadCapabilities={async () => dshSnapshot}
        runHarnessAction={vi.fn() as never} />,
    );
    await screen.findByText('DeepSeek Harness 0.2.0-rc.2 · acp profile');
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    for (const checkbox of screen.getAllByRole('checkbox')) expect(checkbox).toBeDisabled();
  });
});

describe('DeepSeek Harness settings', () => {
  it('keeps a refused setting visible as an error and restores the stored value', async () => {
    const view = { ns: 'agent-loop', revision: 2, fields: [{ key: 'maxParallelToolCalls', type: 'number', value: null, overridden: false }] };
    const runHarnessAction = vi.fn(async (action: { kind: string }) => {
      if (action.kind === 'settings') return { settings: [view] };
      throw new Error('settings namespace "agent-loop" changed since it was read');
    });
    const snapshot = { negotiated: { harness: { kind: 'dsh', version: '0.2.0-rc.2', profile: 'acp', permissionPresets: [], plugins: [], bundles: [], providers: [] } } } as unknown as AgentCapabilitySnapshotDto;
    render(
      <HarnessSettingsFields thread={thread} models={models} busy={false}
        onChange={vi.fn(async () => {})} loadCapabilities={async () => snapshot}
        runHarnessAction={runHarnessAction as never} />,
    );
    await screen.findByText('DeepSeek Harness 0.2.0-rc.2 · acp profile');
    fireEvent.click(screen.getByText('Profile settings'));
    fireEvent(screen.getByText('Profile settings').closest('details')!, new Event('toggle'));
    const input = await screen.findByRole('spinbutton', { name: 'agent-loop.maxParallelToolCalls' });
    fireEvent.change(input, { target: { value: '4' } });
    fireEvent.blur(input);
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({
      kind: 'updateSetting', ns: 'agent-loop', key: 'maxParallelToolCalls', value: 4, revision: 2,
    }));
    expect(await screen.findByRole('alert')).toHaveTextContent('changed since it was read');
    await waitFor(() => expect(screen.getByRole('spinbutton', { name: 'agent-loop.maxParallelToolCalls' })).toHaveValue(null));
  });
});

describe('DeepSeek Harness plugin controls', () => {
  const runModes = [
    { id: 'standard', name: null, description: null, isDefault: true, broken: null },
    { id: 'ptc', name: null, description: null, isDefault: false, broken: null },
    { id: 'minimal', name: null, description: null, isDefault: false, broken: null },
    { id: 'mine', name: 'My mode', description: 'Custom tools', isDefault: false, broken: null },
  ];
  const base = {
    kind: 'dsh', version: '0.2.0-rc.2', profile: 'acp', composition: 'native',
    permissionPresets: [], plugins: [], bundles: [], providers: [], runModes,
    features: { runModes: true, console: true, commands: true },
  };
  const live = (session: Record<string, unknown>) => ({
    ...base,
    session: { running: false, presetLocked: false, commands: [], projections: { agentPreset: 'standard' }, ...session },
  });
  const panel = (harness: unknown, runHarnessAction: unknown, extra: Record<string, unknown> = {}) => render(
    <HarnessSettingsFields thread={thread} models={models} busy={false}
      onChange={vi.fn(async () => {})}
      loadCapabilities={async () => ({ negotiated: { harness } }) as unknown as AgentCapabilitySnapshotDto}
      runHarnessAction={runHarnessAction as never} {...extra} />,
  );

  it('switches the run mode before the first turn and explains the lock after it', async () => {
    const runHarnessAction = vi.fn(async (action: { kind: string; id?: string }) => ({
      result: { agentPreset: action.id },
      harness: live({ projections: { agentPreset: action.id } }),
    }));
    const view = panel(live({}), runHarnessAction);
    const select = await screen.findByRole('combobox', { name: 'Run mode' });
    expect(select).toHaveValue('standard');
    expect(Array.from(select.querySelectorAll("option"), (option) => option.textContent))
      .toEqual(['Standard (default)', 'PTC', 'Minimal', 'My mode']);
    fireEvent.change(select, { target: { value: 'minimal' } });
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'selectRunMode', id: 'minimal' }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: 'Run mode' })).toHaveValue('minimal'));
    expect(screen.getByText(/Only a persistent terminal/)).toBeInTheDocument();
    view.unmount();

    panel(live({ presetLocked: true, projections: { agentPreset: 'ptc' } }), vi.fn());
    expect(await screen.findByRole('combobox', { name: 'Run mode' })).toBeDisabled();
    expect(screen.getByText(/Fixed once the first turn starts/)).toBeInTheDocument();
  });

  it('shows a mode that cannot run on the device as disabled with its reason', async () => {
    const broken = runModes.map((mode) => mode.id === 'ptc' ? { ...mode, broken: 'needs Node.js with TypeScript support' } : mode);
    panel({ ...live({}), runModes: broken }, vi.fn());
    const select = await screen.findByRole('combobox', { name: 'Run mode' });
    const ptc = Array.from(select.querySelectorAll('option')).find((option) => option.value === 'ptc')!;
    expect(ptc).toBeDisabled();
    expect(ptc.textContent).toBe('PTC (unavailable: needs Node.js with TypeScript support)');
  });

  it('runs DSH and plugin commands but leaves thread-owned ones to the thread', async () => {
    const commands = [
      { name: 'plan', description: 'Plan mode', hint: null },
      { name: 'compact', description: 'Compact history', hint: null },
      { name: 'export', description: 'Download the log', hint: null },
      { name: 'review', description: 'Review a path', hint: '<path>' },
    ];
    const runHarnessAction = vi.fn(async () => ({
      result: { commandId: 'c1', result: { kind: 'success', text: 'Reviewed 3 files' } },
      harness: live({ commands }),
    }));
    panel(live({ commands }), runHarnessAction);
    const section = await screen.findByRole('region', { name: 'Commands' });
    expect(section).toHaveTextContent('/planThread control');
    expect(section).toHaveTextContent('/compactThread control');
    expect(section).toHaveTextContent('/exportIn the native console');
    expect(screen.queryByRole('button', { name: 'Run /plan' })).toBeNull();
    fireEvent.change(screen.getByRole('textbox', { name: 'Arguments for /review' }), { target: { value: ' src ' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run /review' }));
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'command', line: '/review src' }));
    expect(await screen.findByText('Reviewed 3 files')).toBeInTheDocument();
  });

  it('asks before /feedback uploads the session to DeepSeek', async () => {
    const commands = [{ name: 'feedback', description: 'Record feedback about this session', hint: '<text>' }];
    const runHarnessAction = vi.fn(async () => ({ result: { result: { kind: 'success', text: 'Feedback recorded' } }, harness: live({ commands }) }));
    panel(live({ commands }), runHarnessAction);
    expect(await screen.findByText(/history to DeepSeek, unless DSH telemetry is turned off/)).toBeInTheDocument();
    fireEvent.change(screen.getByRole('textbox', { name: 'Arguments for /feedback' }), { target: { value: 'great' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run /feedback' }));
    // The app's own confirmation, not window.confirm.
    const dialog = await screen.findByRole('dialog', { name: '/feedback' });
    expect(dialog).toHaveTextContent(/history to DeepSeek/);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog', { name: '/feedback' })).toBeNull());
    expect(runHarnessAction).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Run /feedback' }));
    fireEvent.click(within(await screen.findByRole('dialog', { name: '/feedback' })).getByRole('button', { name: 'Send' }));
    await waitFor(() => expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'command', line: '/feedback great' }));
  });

  it('opens the native console in a reserved tab and closes it when the address fails', async () => {
    const tab = { opener: {} as unknown, location: { replace: vi.fn() }, close: vi.fn() };
    const open = vi.spyOn(window, 'open').mockReturnValue(tab as unknown as Window);
    const runHarnessAction = vi.fn(async () => ({ console: { port: 4242, path: '/?token=launch' } }));
    const dshConsoleUrl = vi.fn(async (target: { port: number; path: string }) =>
      `https://p-1.example${target.path}&__rc_launch=ticket`);
    try {
      const view = panel(live({}), runHarnessAction, { dshConsoleUrl });
      fireEvent.click(await screen.findByRole('button', { name: 'Open console' }));
      expect(open).toHaveBeenCalledWith('about:blank', '_blank');
      expect(tab.opener).toBeNull();
      await waitFor(() => expect(tab.location.replace).toHaveBeenCalledWith('https://p-1.example/?token=launch&__rc_launch=ticket'));
      expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'console' });
      expect(dshConsoleUrl).toHaveBeenCalledWith({ port: 4242, path: '/?token=launch' });
      view.unmount();

      panel(live({}), runHarnessAction, { dshConsoleUrl: async () => { throw new Error('Open Pockymoe on the device'); } });
      fireEvent.click(await screen.findByRole('button', { name: 'Open console' }));
      await waitFor(() => expect(tab.close).toHaveBeenCalled());
      expect(await screen.findByRole('alert')).toHaveTextContent('Open Pockymoe on the device');
    } finally {
      open.mockRestore();
    }
  });

  it('keeps DSH panel actions read-only for thread controllers who do not own the device', async () => {
    const runHarnessAction = vi.fn();
    panel(live({ commands: [{ name: 'review', description: 'Review', hint: null }] }), runHarnessAction,
      { harnessReadOnly: true, dshConsoleUrl: vi.fn() });
    expect(await screen.findByRole('combobox', { name: 'Run mode' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Run /review' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Open console' })).toBeDisabled();
    // The thread's own settings stay editable for them.
    expect(screen.getByRole('combobox', { name: 'Workspace permissions' })).toBeEnabled();
  });

  it('explains a plain ACP fallback and hides the console there', async () => {
    panel({ ...live({}), composition: 'acp', compositionError: 'web bundle failed', runModes: [], features: {} }, vi.fn(), { dshConsoleUrl: vi.fn() });
    expect(await screen.findByText(/no run modes or native console here/)).toBeVisible();
    // The technical reason sits on a small mark after the explanation.
    fireEvent.click(screen.getByRole('button', { name: 'Why it is unavailable' }));
    expect(screen.getByRole('dialog', { name: 'Why it is unavailable' })).toHaveTextContent('web bundle failed');
    expect(screen.queryByRole('button', { name: 'Open console' })).toBeNull();
  });
});

describe('DeepSeek Harness workbench panel', () => {
  it('starts the session through a harness action when the snapshot has no live DSH data', async () => {
    const harness = {
      kind: 'dsh', version: '0.2.0', profile: 'acp', permissionPresets: [], plugins: [], bundles: [], providers: [],
      session: { running: false, presetLocked: false, commands: [], projections: {} },
    };
    const runHarnessAction = vi.fn(async () => ({ result: null, harness }));
    render(<DshPluginPanel loadCapabilities={async () => ({ negotiated: null })} readOnly={false}
      runHarnessAction={runHarnessAction as never} />);
    expect(await screen.findByText('DeepSeek Harness 0.2.0 · acp profile')).toBeInTheDocument();
    expect(runHarnessAction).toHaveBeenCalledWith({ kind: 'refresh' });
  });

  it('never starts a session for viewers', async () => {
    const runHarnessAction = vi.fn();
    render(<DshPluginPanel loadCapabilities={async () => ({ negotiated: null })} readOnly
      runHarnessAction={runHarnessAction as never} />);
    expect(await screen.findByText("This thread's DSH session is not running yet.")).toBeInTheDocument();
    expect(runHarnessAction).not.toHaveBeenCalled();
  });
});
