import { translate, type TranslationKey } from '@pockymoe/thread-ui/i18n';

export type TourLayout = 'desktop' | 'mobile';
export type TourMode = 'local' | 'relay';
/** Pages a step can live on. The tour navigates to plain pages; threads need one the user opened. */
export type TourPage = 'workspaces' | 'workspaceNew' | 'threadImport' | 'relayDevices' | 'thread';
export type TourChapterId = 'devices' | 'threads' | 'terminal' | 'split' | 'files' | 'more';

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
  targets?: TourTargetVariant[];
  /** Shown when no target is visible: what the user must do first. */
  prerequisite?: TranslationKey;
  /** The real control that satisfies the prerequisite, highlighted instead. */
  prerequisiteTarget?: TourSelector;
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
// The one-handed phone layout moves controls into the bottom dock and its More sheet.
const DOCK = 'nav.matter-phone-dock';
const DOCK_MORE = `${DOCK} button.matter-phone-more`;
const DOCK_THREAD = `${DOCK} button.matter-phone-thread`;
const OPEN_SHEET = 'section.matter-phone-sheet[data-open]';

/** Any of the selectors; the first visible match is used. */
const either = (...selectors: TourSelector[]): TourSelector => () => selectors.map(resolveSelector).join(', ');
/** On a phone with the dock, a control inside More: open More first. */
const inMore = { prerequisite: 'tour.openMore.prereq' as const, prerequisiteTarget: DOCK_MORE };

const terminalToggle = (layout: TourLayout) => layout === 'mobile'
  ? either(labelled('button', 'workbench.terminal', TOPBAR), labelled('button', 'workbench.terminal', DOCK))
  : labelled('button', 'workbench.terminal', RAIL);
const explorerToggle = (layout: TourLayout) => layout === 'mobile'
  ? either(labelled('button', 'workbench.toggleExplorer', TOPBAR), labelled('button', 'workbench.toggleExplorer', DOCK))
  : labelled('button', 'workbench.toggleExplorer', RAIL);
const terminalPanel = '[data-testid="workbench-bottom-panel"] [data-testid="terminal-panel"]';

const workspaceSteps: TourStep[] = [
  {
    id: 'local-device',
    page: 'workspaces',
    title: 'tour.devices.local.title',
    body: 'tour.devices.local.body',
    targets: [{ selector: 'header.product-topbar' }],
    only: { mode: 'local' },
  },
  {
    id: 'relay-add-device',
    page: 'relayDevices',
    title: 'tour.devices.relayAdd.title',
    body: 'tour.devices.relayAdd.body',
    targets: [{ selector: 'button[aria-controls="add-device-form"]' }],
    advanceOnClick: true,
    only: { mode: 'relay' },
  },
  {
    id: 'relay-device-form',
    page: 'relayDevices',
    title: 'tour.devices.relayForm.title',
    body: 'tour.devices.relayForm.body',
    targets: [{ selector: '#add-device-form' }],
    prerequisite: 'tour.devices.relayForm.prereq',
    prerequisiteTarget: 'button[aria-controls="add-device-form"]',
    only: { mode: 'relay' },
  },
  {
    id: 'relay-device-list',
    page: 'relayDevices',
    title: 'tour.devices.relayList.title',
    body: 'tour.devices.relayList.body',
    targets: [{ selector: 'section[aria-labelledby="devices-heading"] article:has(.device-presence-dot)' }],
    prerequisite: 'tour.devices.relayList.prereq',
    only: { mode: 'relay' },
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
  {
    id: 'workspace-list',
    page: 'workspaces',
    title: 'tour.devices.list.title',
    body: 'tour.devices.list.body',
    targets: [{ selector: () => `${labelled('section', 'files.workspaceRegistry')()} article.product-row` }],
    prerequisite: 'tour.devices.list.prereq',
  },
];

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
    id: 'model',
    page: 'thread',
    title: 'tour.threads.model.title',
    body: 'tour.threads.model.body',
    targets: [{ selector: '[data-testid="primary-pane"] [data-testid="composer-model-label"]' }],
    prerequisite: 'tour.threads.model.prereq',
  },
  {
    id: 'prompt',
    page: 'thread',
    title: 'tour.threads.prompt.title',
    body: 'tour.threads.prompt.body',
    targets: [{ selector: labelled('[role="textbox"]', 'chat.prompt', '[data-testid="primary-pane"]') }],
  },
  {
    id: 'send',
    page: 'thread',
    title: 'tour.threads.send.title',
    body: 'tour.threads.send.body',
    targets: [{ selector: labelled('button[type="submit"]', 'chat.sendPrompt', '[data-testid="primary-pane"]') }],
  },
  {
    id: 'stop',
    page: 'thread',
    title: 'tour.threads.stop.title',
    body: 'tour.threads.stop.body',
    targets: [{ selector: '[data-testid="primary-pane"] button.thread-graph-composer-stop-button' }],
    prerequisite: 'tour.threads.stop.prereq',
    prerequisiteTarget: labelled('button[type="submit"]', 'chat.sendPrompt', '[data-testid="primary-pane"]'),
  },
  {
    id: 'search',
    page: 'thread',
    title: 'tour.threads.search.title',
    body: 'tour.threads.search.body',
    targets: [
      { selector: `${TOPBAR} button.matter-search-trigger` },
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
      id: 'resize',
      page: 'thread',
      title: 'tour.terminal.resize.title',
      body: 'tour.terminal.resize.body',
      targets: [{ selector: '[data-testid="workbench-panel-sash"]' }],
      ...needsPanel,
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
    targets: [{ selector: '[data-testid="workbench-thread-picker"]' }],
    prerequisite: 'tour.split.picker.prereq',
    prerequisiteTarget: either(splitTrigger, DOCK_MORE),
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
    prerequisiteTarget: either(splitTrigger, DOCK_MORE),
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
    prerequisiteTarget: either(splitTrigger, DOCK_MORE),
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
      prerequisiteTarget: `${FILES} [role="tree"]`,
    },
    {
      id: 'save',
      page: 'thread',
      title: 'tour.files.save.title',
      body: 'tour.files.save.body',
      targets: [
        { selector: `${FILES} [data-testid="workspace-document-conflict"]` },
        { selector: `${FILES} .workspace-file-state` },
      ],
      prerequisite: 'tour.files.edit.prereq',
      prerequisiteTarget: `${FILES} [role="tree"]`,
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
    targets: [{ selector: () => `button.matter-watches-toggle[aria-expanded]:not([aria-label=${quoted(translate('automation.title'))}])` }],
    prerequisite: 'tour.more.subagents.prereq',
    prerequisiteTarget: either(automationToggle, DOCK_MORE),
  },
];

export const TOUR_CHAPTER_IDS: TourChapterId[] = ['devices', 'threads', 'terminal', 'split', 'files', 'more'];

export function tourChapter(id: TourChapterId, context: TourContext): TourChapter {
  const chapters: Record<TourChapterId, Omit<TourChapter, 'id'>> = {
    devices: { title: 'tour.chapter.devices', hint: 'tour.chapter.devicesHint', steps: workspaceSteps },
    threads: { title: 'tour.chapter.threads', hint: 'tour.chapter.threadsHint', steps: threadSteps },
    terminal: { title: 'tour.chapter.terminal', hint: 'tour.chapter.terminalHint', steps: terminalSteps(context.layout) },
    split: { title: 'tour.chapter.split', hint: 'tour.chapter.splitHint', steps: splitSteps },
    files: { title: 'tour.chapter.files', hint: 'tour.chapter.filesHint', steps: fileSteps(context.layout) },
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
