import { translate, type TranslationKey } from '@pockymoe/thread-ui/i18n';

export type TourLayout = 'desktop' | 'mobile';
export type TourMode = 'local' | 'relay';
/**
 * Pages a step can live on. All but the device list are the sample device's
 * (`/devices/pockymoe-tour/...`), so every account sees the same workspace and
 * conversations; the device list is the account's own, in Relay mode.
 */
export type TourPage = 'workspaces' | 'workspaceNew' | 'threadImport' | 'relayDevices' | 'thread';
/** The sample conversation a thread step opens. */
export type TourThread = 'main' | 'helper';
export type TourChapterId = 'devices' | 'threads' | 'terminal' | 'split' | 'files' | 'agents' | 'sharing' | 'more';

export interface TourContext {
  layout: TourLayout;
  mode: TourMode;
}

/** A selector, or a function that builds one for the current locale. */
export type TourSelector = string | (() => string);

/** The first variant whose element is visible is highlighted with its text. */
export interface TourTargetVariant {
  selector: TourSelector;
  body?: TranslationKey;
}

export interface TourStep {
  id: string;
  title: TranslationKey;
  body: TranslationKey;
  page?: TourPage;
  /** For thread steps: which sample conversation (the main one by default). */
  thread?: TourThread;
  targets?: TourTargetVariant[];
  /** Shown when no target is visible: what the user must do first. */
  prerequisite?: TranslationKey;
  /** The real control that satisfies the prerequisite, highlighted instead. */
  prerequisiteTarget?: TourSelector;
  /** The step reads well without its target (e.g. an empty device list): no "waiting". */
  optional?: boolean;
  /** A surrounding area the card must not cover, e.g. the terminal the user should tap. */
  keepClear?: TourSelector;
  /** Clicking the highlighted control moves to the next step. */
  advanceOnClick?: boolean;
  only?: Partial<TourContext>;
}

export interface TourChapter {
  id: TourChapterId;
  title: TranslationKey;
  hint: TranslationKey;
  steps: TourStep[];
}

function quoted(value: string) {
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/** Matches an element by its translated accessible label. */
export function labelled(tag: string, key: TranslationKey, scope = '') {
  return () => `${scope ? `${scope} ` : ''}${tag}[aria-label=${quoted(translate(key))}]`;
}

export function resolveSelector(selector: TourSelector) {
  return typeof selector === 'function' ? selector() : selector;
}

const RAIL = 'nav.matter-rail';
const TOPBAR = 'header.matter-topbar';
const FILES = 'aside.workbench-tool-drawer';
const PRIMARY = '[data-testid="primary-pane"]';
// The one-handed phone layout moves controls into the bottom dock and its More sheet.
const DOCK = 'nav.matter-phone-dock';
const DOCK_MORE = `${DOCK} button.matter-phone-more`;
const DOCK_THREAD = `${DOCK} button.matter-phone-thread`;
const OPEN_SHEET = 'section.matter-phone-sheet[data-open]';

/** Any of the selectors; the first visible match is used. */
const either = (...selectors: TourSelector[]): TourSelector => () => selectors.map(resolveSelector).join(', ');
// On a phone these live in More; on the desktop, in the thread tools menu.
const TOOLS_TOGGLE = '.matter-tabs-row button.matter-toolbar-toggle';
const inMore = { prerequisite: 'tour.openMore.prereq' as const, prerequisiteTarget: either(DOCK_MORE, TOOLS_TOGGLE) };

const terminalToggle = (layout: TourLayout) => layout === 'mobile'
  ? either(labelled('button', 'workbench.terminal', TOPBAR), labelled('button', 'workbench.terminal', DOCK))
  : labelled('button', 'workbench.terminal', RAIL);
const explorerToggle = (layout: TourLayout) => layout === 'mobile'
  ? either(labelled('button', 'workbench.toggleExplorer', TOPBAR), labelled('button', 'workbench.toggleExplorer', DOCK))
  : labelled('button', 'workbench.toggleExplorer', RAIL);
const terminalPanel = '[data-testid="workbench-bottom-panel"] [data-testid="terminal-panel"]';

const workspaceSteps: TourStep[] = [
  {
    id: 'relay-add-device',
    page: 'relayDevices',
    title: 'tour.devices.relayAdd.title',
    body: 'tour.devices.relayAdd.body',
    targets: [{ selector: 'button[aria-controls="add-device-form"]' }],
    advanceOnClick: true,
    only: { mode: 'relay', layout: 'mobile' },
  },
  // A computer's device page adds a device in a window over the page, where the
  // tour steps aside: one card explains the whole form before it opens.
  {
    id: 'relay-add-device',
    page: 'relayDevices',
    title: 'tour.devices.relayAdd.title',
    body: 'tour.devices.relayAddDesktop.body',
    targets: [{ selector: 'button[aria-controls="add-device-form"]' }],
    advanceOnClick: true,
    only: { mode: 'relay', layout: 'desktop' },
  },
  {
    id: 'relay-device-form',
    page: 'relayDevices',
    title: 'tour.devices.relayForm.title',
    body: 'tour.devices.relayForm.body',
    targets: [{ selector: '#add-device-form' }],
    prerequisite: 'tour.devices.relayForm.prereq',
    prerequisiteTarget: 'button[aria-controls="add-device-form"]',
    only: { mode: 'relay', layout: 'mobile' },
  },
  {
    id: 'relay-device-list',
    page: 'relayDevices',
    title: 'tour.devices.relayList.title',
    body: 'tour.devices.relayList.body',
    targets: [{ selector: 'section[aria-labelledby="devices-heading"] article:has(.device-presence-dot), .pd-device-card' }],
    optional: true,
    only: { mode: 'relay' },
  },
  {
    id: 'sample-device',
    page: 'workspaces',
    title: 'tour.devices.sample.title',
    body: 'tour.devices.sample.body',
    targets: [{ selector: 'header.product-topbar' }],
  },
  {
    id: 'local-device',
    page: 'workspaces',
    title: 'tour.devices.local.title',
    body: 'tour.devices.local.body',
    targets: [{ selector: 'header.product-topbar' }],
    only: { mode: 'local' },
  },
  {
    id: 'workspace-list',
    page: 'workspaces',
    title: 'tour.devices.list.title',
    body: 'tour.devices.list.body',
    targets: [{ selector: () => `${labelled('section', 'files.workspaceRegistry')()} article.product-row` }],
  },
  {
    id: 'add-workspace',
    page: 'workspaces',
    title: 'tour.devices.add.title',
    body: 'tour.devices.add.body',
    targets: [{ selector: labelled('a', 'files.addWorkspace') }],
    advanceOnClick: true,
  },
  {
    id: 'workspace-source',
    page: 'workspaceNew',
    title: 'tour.devices.source.title',
    body: 'tour.devices.source.body',
    targets: [{ selector: labelled('div', 'files.workspaceSource') }],
  },
  {
    id: 'workspace-form',
    page: 'workspaceNew',
    title: 'tour.devices.form.title',
    body: 'tour.devices.form.body',
    targets: [{ selector: 'form:has(#workspace-target) button[type="submit"]' }],
  },
];

const composerSend = labelled('button[type="submit"]', 'chat.sendPrompt', PRIMARY);
const workLine = `${PRIMARY} .thread-worked-toggle`;

const threadSteps: TourStep[] = [
  {
    id: 'import',
    page: 'workspaces',
    title: 'tour.threads.import.title',
    body: 'tour.threads.import.body',
    targets: [{ selector: labelled('a', 'files.importSession') }],
    advanceOnClick: true,
  },
  {
    id: 'import-form',
    page: 'threadImport',
    title: 'tour.threads.importForm.title',
    body: 'tour.threads.importForm.body',
    targets: [{ selector: 'form:has(#backend-provider)' }],
  },
  {
    id: 'history',
    page: 'thread',
    title: 'tour.threads.history.title',
    body: 'tour.threads.history.body',
    targets: [
      { selector: 'aside.matter-sidebar.is-open' },
      { selector: labelled('button', 'workbench.toggleShortcutsSidebar', TOPBAR), body: 'tour.threads.historyMobile.body' },
      { selector: DOCK_THREAD, body: 'tour.threads.historyDock.body' },
    ],
    only: { layout: 'mobile' },
  },
  {
    id: 'history',
    page: 'thread',
    title: 'tour.threads.history.title',
    body: 'tour.threads.history.body',
    targets: [{ selector: 'aside.matter-sidebar' }],
    prerequisite: 'tour.threads.historyMobile.body',
    prerequisiteTarget: labelled('button', 'workbench.toggleShortcutsSidebar', TOPBAR),
    only: { layout: 'desktop' },
  },
  {
    id: 'new-thread',
    page: 'thread',
    title: 'tour.threads.new.title',
    body: 'tour.threads.new.body',
    targets: [{ selector: 'button.matter-new-thread' }],
    // On the dock it is at the foot of the thread list.
    prerequisite: 'tour.threads.newThread.prereqDock',
    prerequisiteTarget: DOCK_THREAD,
  },
  {
    id: 'work',
    page: 'thread',
    title: 'tour.threads.work.title',
    body: 'tour.threads.work.body',
    targets: [{ selector: workLine }],
  },
  {
    id: 'prompt',
    page: 'thread',
    title: 'tour.threads.prompt.title',
    body: 'tour.threads.prompt.body',
    targets: [{ selector: labelled('[role="textbox"]', 'chat.prompt', PRIMARY) }],
  },
  {
    id: 'model',
    page: 'thread',
    title: 'tour.threads.model.title',
    body: 'tour.threads.model.body',
    targets: [{ selector: `${PRIMARY} [data-testid="composer-model-label"]` }],
    // A compact composer shows its model once it is in use.
    prerequisite: 'tour.threads.model.prereq',
    prerequisiteTarget: labelled('[role="textbox"]', 'chat.prompt', PRIMARY),
  },
  {
    id: 'send',
    page: 'thread',
    title: 'tour.threads.send.title',
    body: 'tour.threads.send.body',
    targets: [{ selector: composerSend }],
    advanceOnClick: true,
  },
  {
    id: 'stop',
    page: 'thread',
    title: 'tour.threads.stop.title',
    body: 'tour.threads.stop.body',
    targets: [{ selector: `${PRIMARY} button.thread-graph-composer-stop-button` }],
    prerequisite: 'tour.threads.stop.prereq',
    prerequisiteTarget: composerSend,
  },
  {
    id: 'quote',
    page: 'thread',
    title: 'tour.threads.quote.title',
    body: 'tour.threads.quote.body',
    targets: [
      { selector: 'button.quote-selection-pill', body: 'tour.threads.quotePill.body' },
      { selector: `${PRIMARY} .thread-timeline-surface [data-testid="chat-message"][data-role="assistant"]:last-of-type` },
      { selector: `${PRIMARY} [data-testid="chat-message"][data-role="assistant"]` },
    ],
  },
  {
    id: 'search',
    page: 'thread',
    title: 'tour.threads.search.title',
    body: 'tour.threads.search.body',
    targets: [
      { selector: `${TOPBAR} button.matter-search-trigger` },
      { selector: '.matter-tabs-search button.matter-search-trigger' },
      { selector: `${OPEN_SHEET} .matter-phone-sheet-tools button:has([data-icon="search"])` },
    ],
    ...inMore,
  },
];

function terminalSteps(layout: TourLayout): TourStep[] {
  const needsPanel = {
    prerequisite: 'tour.terminal.needsPanel' as const,
    prerequisiteTarget: terminalToggle(layout),
  };
  return [
    {
      id: 'open',
      page: 'thread',
      title: 'tour.terminal.open.title',
      body: layout === 'mobile' ? 'tour.terminal.openMobile.body' : 'tour.terminal.open.body',
      targets: [{ selector: terminalToggle(layout) }],
      advanceOnClick: true,
    },
    {
      id: 'type',
      page: 'thread',
      title: 'tour.terminal.type.title',
      body: 'tour.terminal.type.body',
      targets: [{ selector: `${terminalPanel} .xterm` }],
      keepClear: '[data-testid="workbench-bottom-panel"]',
      ...needsPanel,
    },
    {
      id: 'resize',
      page: 'thread',
      title: 'tour.terminal.resize.title',
      body: 'tour.terminal.resize.body',
      targets: [{ selector: '[data-testid="workbench-panel-sash"]' }],
      ...needsPanel,
      only: { layout: 'desktop' },
    },
    {
      id: 'new',
      page: 'thread',
      title: 'tour.terminal.new.title',
      body: 'tour.terminal.new.body',
      targets: [{ selector: `${terminalPanel} [data-testid="terminal-new"]` }],
      ...needsPanel,
    },
    {
      id: 'tabs',
      page: 'thread',
      title: 'tour.terminal.tabs.title',
      body: 'tour.terminal.tabs.body',
      targets: [
        { selector: `${terminalPanel} [data-testid="terminal-tabs"]` },
        { selector: `${terminalPanel} button.terminal-switcher`, body: 'tour.terminal.switcher.body' },
      ],
      prerequisite: 'tour.terminal.tabs.prereq',
      prerequisiteTarget: `${terminalPanel} [data-testid="terminal-new"]`,
    },
    {
      id: 'split',
      page: 'thread',
      title: 'tour.terminal.split.title',
      body: 'tour.terminal.split.body',
      targets: [
        { selector: `${terminalPanel} [data-testid="terminal-split"]` },
        { selector: `${terminalPanel} [data-testid="terminal-more"]`, body: 'tour.terminal.more.body' },
      ],
      ...needsPanel,
    },
    {
      id: 'touch',
      page: 'thread',
      title: 'tour.terminal.touch.title',
      body: 'tour.terminal.touch.body',
      targets: [{ selector: `${terminalPanel} .shell-touch-controls` }],
      keepClear: '[data-testid="workbench-bottom-panel"]',
      ...needsPanel,
      only: { layout: 'mobile' },
    },
    {
      id: 'target',
      page: 'thread',
      title: 'tour.terminal.target.title',
      body: 'tour.terminal.target.body',
      targets: [{ selector: `${terminalPanel} [data-testid="terminal-target"]` }],
      ...needsPanel,
      // Compact terminal headers hide the target label.
      only: { layout: 'desktop' },
    },
    {
      id: 'hide',
      page: 'thread',
      title: 'tour.terminal.hide.title',
      body: 'tour.terminal.hide.body',
      targets: [{ selector: `${terminalPanel} [data-testid="workbench-close-tools"]` }],
      ...needsPanel,
    },
  ];
}

const splitTrigger = '[data-testid="workbench-split-trigger"]';

const splitSteps: TourStep[] = [
  {
    id: 'trigger',
    page: 'thread',
    title: 'tour.split.trigger.title',
    body: 'tour.split.trigger.body',
    targets: [{ selector: splitTrigger }],
    advanceOnClick: true,
    ...inMore,
  },
  {
    id: 'picker',
    page: 'thread',
    title: 'tour.split.picker.title',
    body: 'tour.split.picker.body',
    targets: [
      { selector: '[data-testid="workbench-thread-picker"]' },
      // Picking a thread closes the picker: the split is open.
      { selector: '[data-testid="workbench-panels"]:not(.is-compact) [data-testid="reference-pane"]', body: 'tour.split.picker.done' },
      { selector: '.thread-jump-latest-cluster.has-pane-switch', body: 'tour.split.picker.done' },
    ],
    prerequisite: 'tour.split.picker.prereq',
    prerequisiteTarget: either(splitTrigger, DOCK_MORE, TOOLS_TOGGLE),
  },
  {
    id: 'panes',
    page: 'thread',
    title: 'tour.split.panes.title',
    body: 'tour.split.panes.body',
    targets: [
      { selector: '[data-testid="workbench-panels"]:not(.is-compact) [data-testid="reference-pane"]' },
      // Phones switch the two conversations beside the composer's timeline capsule.
      { selector: '.thread-jump-latest-cluster.has-pane-switch', body: 'tour.split.mobile.body' },
    ],
    prerequisite: 'tour.split.panes.prereq',
    prerequisiteTarget: either(splitTrigger, DOCK_MORE, TOOLS_TOGGLE),
  },
  {
    id: 'focus',
    page: 'thread',
    title: 'tour.split.focus.title',
    body: 'tour.split.focus.body',
    targets: [{ selector: '[data-testid="make-primary"]' }],
    // Phones have no pane header; the split trigger closes the split.
    only: { layout: 'desktop' },
    prerequisite: 'tour.split.panes.prereq',
    prerequisiteTarget: either(splitTrigger, DOCK_MORE, TOOLS_TOGGLE),
  },
  {
    id: 'tabs',
    page: 'thread',
    title: 'tour.split.tabs.title',
    body: 'tour.split.tabs.body',
    targets: [{ selector: '.matter-tabs-row .matter-thread-group' }],
    only: { layout: 'desktop' },
  },
];

function fileSteps(layout: TourLayout): TourStep[] {
  const needsDrawer = {
    prerequisite: 'tour.files.needsDrawer' as const,
    prerequisiteTarget: explorerToggle(layout),
  };
  return [
    {
      id: 'open',
      page: 'thread',
      title: 'tour.files.open.title',
      body: 'tour.files.open.body',
      targets: [{ selector: explorerToggle(layout) }],
      advanceOnClick: true,
    },
    {
      id: 'tree',
      page: 'thread',
      title: 'tour.files.tree.title',
      body: 'tour.files.tree.body',
      targets: [{ selector: `${FILES} [role="tree"]` }],
      ...needsDrawer,
    },
    {
      id: 'filter',
      page: 'thread',
      title: 'tour.files.filter.title',
      body: 'tour.files.filter.body',
      targets: [{ selector: labelled('button', 'files.filterWorkspace', FILES) }],
      ...needsDrawer,
    },
    {
      id: 'new-file',
      page: 'thread',
      title: 'tour.files.newFile.title',
      body: 'tour.files.newFile.body',
      targets: [{ selector: labelled('button', 'files.newFile', FILES) }],
      ...needsDrawer,
    },
    {
      id: 'edit',
      page: 'thread',
      title: 'tour.files.edit.title',
      body: 'tour.files.edit.body',
      targets: [
        { selector: labelled('button', 'files.saveFile', FILES) },
        { selector: labelled('button', 'files.editFile', FILES) },
      ],
      prerequisite: 'tour.files.edit.prereq',
      // A file's tools sit behind its ⋯; with no file open, the tree.
      prerequisiteTarget: either(`${FILES} button.workspace-file-more[aria-expanded="false"]`, `${FILES} [role="tree"]`),
    },
    {
      id: 'save',
      page: 'thread',
      title: 'tour.files.save.title',
      body: 'tour.files.save.body',
      targets: [
        { selector: `${FILES} [data-testid="workspace-document-conflict"]` },
        { selector: `${FILES} .workspace-file-state` },
        { selector: labelled('button', 'files.saveFile', FILES) },
        { selector: labelled('button', 'files.editFile', FILES) },
      ],
      prerequisite: 'tour.files.edit.prereq',
      // A file's tools sit behind its ⋯; with no file open, the tree.
      prerequisiteTarget: either(`${FILES} button.workspace-file-more[aria-expanded="false"]`, `${FILES} [role="tree"]`),
    },
    {
      id: 'close',
      page: 'thread',
      title: 'tour.files.close.title',
      body: 'tour.files.close.body',
      targets: [{ selector: `${FILES} [data-testid="workbench-close-files"]` }],
      ...needsDrawer,
    },
  ];
}

const agentToggle = either(
  'aside.matter-sidebar button.matter-agent-toggle',
  `${OPEN_SHEET} button.matter-agent-toggle`,
  '.matter-tabs-row button.matter-group-toggle',
);

const agentSteps: TourStep[] = [
  {
    id: 'delegate',
    page: 'thread',
    title: 'tour.agents.delegate.title',
    body: 'tour.agents.delegate.body',
    targets: [{ selector: workLine }],
    advanceOnClick: true,
  },
  {
    id: 'wake',
    page: 'thread',
    title: 'tour.agents.wake.title',
    body: 'tour.agents.wake.body',
    targets: [{ selector: `${PRIMARY} .thread-peer-work-toggle` }],
    prerequisite: 'tour.agents.wake.prereq',
    prerequisiteTarget: workLine,
  },
  {
    id: 'children',
    page: 'thread',
    title: 'tour.agents.children.title',
    body: 'tour.agents.children.body',
    targets: [{ selector: agentToggle }],
    prerequisite: 'tour.agents.children.prereq',
    prerequisiteTarget: either(DOCK_THREAD, labelled('button', 'workbench.toggleShortcutsSidebar', TOPBAR)),
  },
  {
    id: 'envelope',
    page: 'thread',
    thread: 'helper',
    title: 'tour.agents.envelope.title',
    body: 'tour.agents.envelope.body',
    targets: [{ selector: `${PRIMARY} .prompt-envelope-head` }],
    // Leave the task itself readable.
    keepClear: `${PRIMARY} [data-testid="chat-message"]:has(.prompt-envelope-head)`,
  },
  {
    id: 'parent',
    page: 'thread',
    thread: 'helper',
    title: 'tour.agents.parent.title',
    body: 'tour.agents.parent.body',
    targets: [{ selector: `${PRIMARY} .thread-relation-bar` }, { selector: `${PRIMARY} .prompt-envelope-link` }],
  },
  {
    id: 'cli',
    page: 'thread',
    thread: 'helper',
    title: 'tour.agents.cli.title',
    body: 'tour.agents.cli.body',
    targets: [{ selector: labelled('[role="textbox"]', 'chat.prompt', PRIMARY) }],
  },
];

const sharingButton = either(
  labelled('button', 'workbench.sharingPermissions', '#matter-thread-tools'),
  labelled('button', 'workbench.sharingPermissions', OPEN_SHEET),
);
const SHARE_FORM = 'form#thread-actions-share-form';
const needsDialog = { prerequisite: 'tour.sharing.needsDialog' as const, prerequisiteTarget: either(sharingButton, DOCK_MORE, TOOLS_TOGGLE) };

const sharingSteps: TourStep[] = [
  {
    id: 'open',
    page: 'thread',
    title: 'tour.sharing.open.title',
    body: 'tour.sharing.open.body',
    targets: [{ selector: sharingButton }],
    advanceOnClick: true,
    ...inMore,
  },
  {
    id: 'scope',
    page: 'thread',
    title: 'tour.sharing.scope.title',
    body: 'tour.sharing.scope.body',
    targets: [{ selector: `${SHARE_FORM} fieldset.matter-share-scopes` }],
    ...needsDialog,
  },
  {
    id: 'invite',
    page: 'thread',
    title: 'tour.sharing.invite.title',
    body: 'tour.sharing.invite.body',
    targets: [{ selector: `${SHARE_FORM} input[type="text"], ${SHARE_FORM} input:not([type])` }],
    ...needsDialog,
  },
  {
    id: 'access',
    page: 'thread',
    title: 'tour.sharing.access.title',
    body: 'tour.sharing.access.body',
    // The first box is thread access; files follow it.
    targets: [{ selector: `${SHARE_FORM} fieldset.thread-export-dialog-box` }],
    ...needsDialog,
  },
  {
    id: 'submit',
    page: 'thread',
    title: 'tour.sharing.submit.title',
    body: 'tour.sharing.submit.body',
    targets: [{ selector: `button[type="submit"][form="thread-actions-share-form"]` }],
    ...needsDialog,
  },
  {
    id: 'elsewhere',
    page: 'thread',
    title: 'tour.sharing.elsewhere.title',
    body: 'tour.sharing.elsewhere.body',
    targets: [{ selector: labelled('button', 'workbench.shareAsLink', '#matter-thread-tools') }, { selector: labelled('button', 'workbench.shareAsLink', OPEN_SHEET) }],
    optional: true,
  },
];

const automationToggle = labelled('button.matter-watches-toggle', 'automation.title');

const moreSteps: TourStep[] = [
  {
    id: 'automation',
    page: 'thread',
    title: 'tour.more.automation.title',
    body: 'tour.more.automation.body',
    targets: [{ selector: automationToggle }],
    ...inMore,
  },
  {
    id: 'subagents',
    page: 'thread',
    title: 'tour.more.subagents.title',
    body: 'tour.more.subagents.body',
    targets: [{ selector: () => `button.matter-watches-toggle[aria-expanded]:not([aria-label=${quoted(translate('automation.title'))}]):not(.thread-plugins-toggle)` }],
    prerequisite: 'tour.more.subagents.prereq',
    prerequisiteTarget: either(automationToggle, DOCK_MORE, TOOLS_TOGGLE),
  },
  {
    id: 'notifications',
    page: 'thread',
    title: 'tour.more.notifications.title',
    body: 'tour.more.notifications.body',
    targets: [{ selector: labelled('button', 'workbench.notifications') }],
  },
  {
    id: 'finish',
    page: 'thread',
    title: 'tour.more.finish.title',
    body: 'tour.more.finish.body',
    optional: true,
  },
];

export const TOUR_CHAPTER_IDS: TourChapterId[] = ['devices', 'threads', 'terminal', 'split', 'files', 'agents', 'sharing', 'more'];

export function tourChapter(id: TourChapterId, context: TourContext): TourChapter {
  const chapters: Record<TourChapterId, Omit<TourChapter, 'id'>> = {
    devices: { title: 'tour.chapter.devices', hint: 'tour.chapter.devicesHint', steps: workspaceSteps },
    threads: { title: 'tour.chapter.threads', hint: 'tour.chapter.threadsHint', steps: threadSteps },
    terminal: { title: 'tour.chapter.terminal', hint: 'tour.chapter.terminalHint', steps: terminalSteps(context.layout) },
    split: { title: 'tour.chapter.split', hint: 'tour.chapter.splitHint', steps: splitSteps },
    files: { title: 'tour.chapter.files', hint: 'tour.chapter.filesHint', steps: fileSteps(context.layout) },
    agents: { title: 'tour.chapter.agents', hint: 'tour.chapter.agentsHint', steps: agentSteps },
    sharing: { title: 'tour.chapter.sharing', hint: 'tour.chapter.sharingHint', steps: sharingSteps },
    more: { title: 'tour.chapter.more', hint: 'tour.chapter.moreHint', steps: moreSteps },
  };
  const chapter = chapters[id];
  return {
    id,
    ...chapter,
    steps: chapter.steps.filter(
      (step) =>
        (!step.only?.layout || step.only.layout === context.layout) &&
        (!step.only?.mode || step.only.mode === context.mode),
    ),
  };
}
