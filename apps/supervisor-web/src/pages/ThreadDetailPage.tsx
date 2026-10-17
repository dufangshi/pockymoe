import { useBrowserStateKey } from '../lib/browserStateScope';
import { TRANSCRIPT_PREFERENCE_EVENT } from '../lib/transcriptPreferences';
import { SupervisorHeartbeat, SUPERVISOR_HEARTBEAT_TIMEOUT_MS } from '../lib/supervisorHeartbeat';
import { appUrl } from '../lib/webBase';
import { workbenchUrls } from '../lib/workbenchUrls';
import { HostFeatureGate, useHostScope, UnscopedOnly } from '../lib/hostCapabilities';
import { sharingEditInput, type SharingInput } from '../lib/signedSharing';
import { SignedShareDetails } from '../components/SignedShareDetails';
import { useThreadDrafts } from './useThreadDrafts';
import { useWorkbenchReference } from './useWorkbenchReference';
import { WorkbenchThreadPicker } from '../components/WorkbenchThreadPicker';
import { WorkbenchTerminal, terminalTargetKey, type TerminalOpenRequest } from '../components/WorkbenchTerminal';
import { WorkbenchReferencePane } from '../components/WorkbenchReferencePane';
import { WorkbenchCollaboration } from '../components/WorkbenchCollaboration';
import { FileAskBar } from '../components/FileAskBar';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { DeviceMonitor } from '../components/DeviceMonitor';
import { TourLauncherButton } from '../components/onboarding/TourProvider';
import { HarnessSettingsDialog, HarnessSettingsFields } from '../components/HarnessSettingsDialog';
import { DshPluginPanel, type DshConsoleTarget, type DshPanelAction, type DshPanelResult } from '../components/DshHarnessPanel';
import { DEEPSEEK_HARNESS_PANEL_KIND } from '@pockymoe/thread-ui/builtin-plugins';
import { ConversationSearch } from '../components/ConversationSearch';
import { useSearchMessages } from '../components/searchMessages';
import { useWorkbenchNavigation } from './useWorkbenchNavigation';
import { useScopedState } from './useScopedState';
import { useSubscriptionUsage } from './useSubscriptionUsage';
import { useThreadTabStatus } from '../lib/useThreadTabStatus';
import { DeviceEncryptionStatus } from '../components/DeviceEncryptionStatus';
import { ThreadPublicLinks } from '../components/ThreadPublicLinks';
import { ThreadWatchesControl } from '../components/ThreadWatchesControl';
import { ThreadSubagentsControl } from '../components/ThreadSubagentsControl';
import { PortMappingsControl } from '../components/PortMappingsControl';
import { startTransition, useCallback, useEffect, useMemo, useRef, useState, type SetStateAction } from 'react';
import { useLocation, useNavigate, useParams } from 'react-router-dom';
import { Download, Link2, Network, Puzzle, Users } from 'lucide-react';

import { RecentThreadMenu } from '../components/RecentThreadMenu';
import {
  AgentProviderCapabilitiesDto,
  AgentBackendManagementSchemaDto,
  AgentRuntimeStatusDto,
  ModelOptionDto,
  RelayEffectiveAccessDto,
  SupervisorSocketServerEnvelope,
  ThreadDetailDto,
  ThreadHistoryItemDto,
  ThreadDto,
  ThreadEventEnvelope,
  ThreadTurnPriceEstimateDto,
  ThreadTurnTokenUsageDto,
  truncateAutoThreadTitle,
} from '../../../../packages/shared/src/index';
import { appSettingsSections } from '../components/AppShellSettingsDialog';
import { useAppShellNav } from '../components/AppShellNavContext';
import {
  ConfirmDialog,
  confirmWorkspaceDocumentLeave,
  LongTextDialog,
  ThreadActionsDialog,
  ThreadDetailSurface,
  ThreadPluginScope,
  ThreadPluginsControl,
  useWorkbenchPresentation,
  ThreadShellPanel,
  ThreadGraphWorkspacePanel,
  ThreadTimeline,
  type ThreadShellControlState,
  type ThreadShellPanelHandle,
  type ThreadComposerProps,
  type CreateThreadShareInput,
  type ThreadShareSummary,
  type ThreadTimelineProps,
  type ThreadGraphWorkspaceFeatures,
} from '@pockymoe/thread-ui';
import {
  formatLongTimestamp,
  threadStatusLabel,
} from '@pockymoe/thread-ui';
import { usePlugins } from '@pockymoe/thread-ui';
import {
  isInvisibleThreadError,
  ApiError,
  compactThread,
  connectSupervisorEvents,
  connectShellSocket,
  createThreadShell,
  createRelayShare,
  createRelayGrant,
  updateRelayGrant,
  revokeRelayGrant,
  disconnectThread,
  deleteThread,
  fetchAgentBackendModels,
  fetchAgentBackendAgents,
  fetchAgentBackendModelsFor,
  fetchThreadCapabilitySnapshot,
  postThreadHarnessAction,
  dshConsoleUrl,
  fetchThreadModels,
  fetchThreadGroup,
  fetchAgentBackendStatus,
  fetchProviderHostFile,
  fetchRelayAccess,
  fetchRelayPortal,
  fetchRelaySession,
  fetchThreadHistoryItemDetail,
  fetchThreadTurnDetail,
  fetchThreadShellState,
  fetchSupervisorHealth,
  fetchThreads,
  fetchThreadDetail,
  fetchThreadDelivery,
  interruptThread,
  respondToThreadRequest,
  revokeRelayShare,
  resumeThread,
  relayModeActive,
  sendThreadPrompt,
  type PromptAttachmentUpload,
  type SendThreadPromptRequestInput,
  updateThread,
  updateRelayShare,
  updateShell,
  updateProviderHostFile,
  updateThreadSettings,
  terminateShell,
  buildThreadImageAssetUrl,
  cancelPendingSteer,
  steerPendingPrompt,
  steerSubmittedPrompt,
  threadPluginAdapter,
} from '../lib/api';
import {
  appendLiveAgentDeltaToItems,
  appendLatestTurns,
  applyLiveItemTimestampsToTurns,
  createClientRequestId,
  findMaterializedOptimisticTurn,
  findTurnWithUserMessage,
  getReasoningEffortAvailability,
  isThreadActionRequest,
  mergeGoalHistory,
  mergeLiveHistoryItem,
  mergePendingRequestIntoDetail,
  mergePendingRequests,
  mergeThreadIntoList,
  mergeTurnTokenUsage,
  normalizeGoalHistory,
  prependTurns,
  reconcileLiveItemsWithDetail,
  resolveThreadContextUsage,
  removePendingRequestFromDetail,
  turnHasPhotoAttachment,
  promptHasPhotoPlaceholder,
  turnHasUserMessage,
} from './threadDetailModel';
import {
  currentNewThreadHref,
  currentRelayDeviceIdFromPath,
  currentThreadHref,
  threadHref,
  currentThreadsHref,
  currentWorkspacesHref,
  relayDeviceIdFromPath,
  threadIdFromPath,
} from '../lib/relayRoutes';
import { useMobileComposerLayout } from './useMobileComposerLayout';
import { useThreadAuxiliaryActions } from './useThreadAuxiliaryActions';
import { useThreadListPolling } from './useThreadListPolling';
import { useEventDrivenRefresh } from '../lib/useEventDrivenRefresh';
import { useThreadView } from '../lib/threadView';
import { announceHarnessRuntimeEvent } from '../lib/harnessRuntimeEvents';
import { useThreadWorkspaceAdapter } from './useThreadWorkspaceAdapter';
import { ThreadCreateForm } from './thread-create/ThreadCreateForm';

const INITIAL_DETAIL_TURN_PAGE_SIZE = 3;
const DETAIL_TURN_PAGE_SIZE = 3;
const SUPERVISOR_SOCKET_RECONNECT_DELAY_MS = 1_000;
const SUPERVISOR_HEALTHCHECK_INTERVAL_MS = 2_000;
const SUPERVISOR_CONNECTION_STALE_MS = SUPERVISOR_HEARTBEAT_TIMEOUT_MS;
/** Summary re-read while a turn runs and no socket delivers its events. */
const ACTIVE_THREAD_REFRESH_INTERVAL_MS = 3_000;
/** Capability snapshot fallback; events and connection changes refresh it sooner. */
const CAPABILITY_FALLBACK_REFRESH_MS = 60_000;
/** ACP agents announce commands shortly after a session opens. */
const CAPABILITY_SETTLE_DELAYS_MS = [1_500, 6_000];
/** Turn end without a following thread.updated (older Supervisors). */
const TURN_END_REFRESH_GRACE_MS = 1_500;
const DETAIL_REFRESH_COALESCE_MS = 150;
// Other threads' events that change what the thread list shows.
const THREAD_LIST_EVENT_TYPES = new Set([
  'thread.turn.started',
  'thread.turn.completed',
  'thread.turn.failed',
  'thread.updated',
]);
// Current-thread events after which the capability snapshot may differ.
const CAPABILITY_EVENT_TYPES = new Set([
  'thread.harness.ready',
  'thread.turn.started',
  'thread.turn.completed',
  'thread.turn.failed',
  'thread.updated',
  'thread.collaboration.updated',
]);
const isPendingSteerUpdate = (event: ThreadEventEnvelope) =>
  event.type === 'thread.updated' && event.payload.reason === 'pending_steer_updated';
const threadIsActive = (detail: ThreadDetailDto | null) =>
  Boolean(detail && (detail.thread.activeTurnId != null || detail.thread.status === 'running'));
const SOCKET_CONNECTING = 0;

const SOCKET_OPEN = 1;
const SOCKET_CLOSED = 3;
const EMPTY_ANSWERED_REQUEST_NOTES: NonNullable<
  ThreadDetailDto['answeredRequestNotes']
> = [];
const EMPTY_ACTIVITY_NOTES: NonNullable<ThreadDetailDto['activityNotes']> = [];
const EMPTY_PENDING_STEERS: NonNullable<ThreadDetailDto['pendingSteers']> = [];
/** The last detail of recently opened threads, by route: switching back paints
    it at once and refreshes quietly, instead of a spinner in place of the
    conversation and the composer. */
const recentThreadDetails = new Map<string, ThreadDetailDto>();
/** Prefetches still in flight, reused by the page that opens the thread. */
const pendingThreadDetails = new Map<string, Promise<ThreadDetailDto>>();
function rememberThreadDetail(routeKey: string, detail: ThreadDetailDto) {
  recentThreadDetails.delete(routeKey);
  recentThreadDetails.set(routeKey, detail);
  while (recentThreadDetails.size > 12) recentThreadDetails.delete(recentThreadDetails.keys().next().value!);
}
const UNAVAILABLE_AGENT_CAPABILITIES: AgentProviderCapabilitiesDto = {
  sessions: {
    list: false,
    read: false,
    resume: false,
    importLocal: false,
    load: false,
    close: false,
    delete: false,
  },
  turns: {
    start: false,
    streamInput: false,
    steer: false,
    interrupt: false,
    compact: false,
  },
  branching: {
    fork: false,
    hardRollback: false,
    resumeAt: false,
    rewindFiles: false,
  },
  controls: {
    planMode: false,
    permissionRequests: false,
    sandboxMode: false,
    performanceMode: false,
    goals: false,
  },
  management: {
    models: false,
    mcpStatus: false,
    skills: false,
    hooks: false,
    hookTrust: false,
    hostConfigFiles: false,
    providerSettings: false,
  },
  usage: {
    contextWindow: false,
    tokenUsage: false,
    costUsd: false,
  },
};
const SUPERVISOR_WORKSPACE_FEATURES: ThreadGraphWorkspaceFeatures = {
  workspace: true,
  toolUsage: false,
  guide: false,
  threadGraph: false,
  extensions: false,
  defaultTab: 'workspace',
};

function actionErrorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : fallback;
}

function relayThreadAccessLabel(access: RelayEffectiveAccessDto['threadAccess']) {
  return access === 'read' ? translate("workbench.viewOnly") : translate("workbench.collaborator");
}

function relayWorkspaceAccessLabel(access: RelayEffectiveAccessDto['workspaceAccess']) {
  switch (access) {
    case 'write':
      return translate("workbench.workspaceWrite");
    case 'read':
      return translate("workbench.workspaceRead");
    case 'none':
    default:
      return translate("workbench.noWorkspace");
  }
}

type RealtimeConnectionStatus =
  | 'checking'
  | 'connected'
  | 'reconnecting'
  | 'offline';


interface RealtimeConnectionSnapshot {
  status: RealtimeConnectionStatus;
  browserOnline: boolean;
  healthOk: boolean;
  socketOpen: boolean;
  lastHealthyAt: string | null;
}

interface OptimisticTurnState {
  id: string;
  serverTurnId: string | null;
  startedAt: string;
  status: 'sending' | 'inProgress' | 'completed' | 'interrupted' | 'failed' | 'recovering';
  error: string | null;
  prompt: string;
  attachmentPreviews: OptimisticAttachmentPreview[];
  model: string | null;
  reasoningEffort: ThreadDetailDto['thread']['reasoningEffort'];
  reasoningEffortAvailable: boolean | null;
  tokenUsage: ThreadTurnTokenUsageDto | null;
  priceEstimate: ThreadTurnPriceEstimateDto | null;
}

interface OptimisticAttachmentPreview {
  path: string;
  url: string;
}

interface OptimisticSteerState {
  id: string;
  clientRequestId: string;
  turnId: string;
  prompt: string;
  createdAt: string;
  status: 'steering' | 'accepted';
}

interface WorkspaceFocusPathRequest {
  path: string;
  line?: number;
  requestId: number;
}

type PendingThreadSettings = Partial<
  Pick<
    ThreadDto,
    'model' | 'reasoningEffort' | 'fastMode' | 'collaborationMode' | 'sandboxMode'
  >
>;

function photoPlaceholderPath(placeholder: string) {
  return placeholder.match(/^\[PHOTO\s+([^\]]+)\]$/)?.[1]?.trim() ?? null;
}

function buildOptimisticAttachmentPreviews(
  attachments: PromptAttachmentUpload[] | undefined,
): OptimisticAttachmentPreview[] {
  if (!attachments?.length || typeof URL.createObjectURL !== 'function') {
    return [];
  }

  return attachments.flatMap((attachment) => {
    if (attachment.kind !== 'photo') {
      return [];
    }

    const path = photoPlaceholderPath(attachment.placeholder);
    if (!path) {
      return [];
    }

    return [
      {
        path,
        url: URL.createObjectURL(attachment.file),
      },
    ];
  });
}

function revokeOptimisticAttachmentPreviews(
  previews: OptimisticAttachmentPreview[],
) {
  if (typeof URL.revokeObjectURL !== 'function') {
    return;
  }

  for (const preview of previews) {
    URL.revokeObjectURL(preview.url);
  }
}

function relativeWorkspaceLinkPath(path: string, workspaceAbsPath: string) {
  const normalizedPath = path.trim().replace(/\\/g, '/').replace(/\/+$/, '');
  const normalizedRoot = workspaceAbsPath.trim().replace(/\\/g, '/').replace(/\/+$/, '');
  if (!normalizedPath) {
    return null;
  }
  if (!normalizedPath.startsWith('/')) {
    return normalizedPath.replace(/^\.\/+/, '').replace(/^\/+/, '');
  }
  if (normalizedPath === normalizedRoot) {
    return '';
  }
  const rootPrefix = `${normalizedRoot}/`;
  if (!normalizedPath.startsWith(rootPrefix)) {
    return null;
  }
  return normalizedPath.slice(rootPrefix.length);
}

function CopyIcon() {
  const { locale: i18nLocale } = useI18n();
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      className="h-3.5 w-3.5 fill-current"
    >
      <path d="M5.75 1.75c-.97 0-1.75.78-1.75 1.75v.25H3.5c-.97 0-1.75.78-1.75 1.75v6c0 .97.78 1.75 1.75 1.75h4.75c.97 0 1.75-.78 1.75-1.75v-.25h.5c.97 0 1.75-.78 1.75-1.75v-6c0-.97-.78-1.75-1.75-1.75h-4.75Zm-.5 2V3.5c0-.28.22-.5.5-.5h4.75c.28 0 .5.22.5.5v6a.5.5 0 0 1-.5.5H10v-4.5c0-.97-.78-1.75-1.75-1.75h-3Zm-1.75 1.25h4.75c.28 0 .5.22.5.5v6a.5.5 0 0 1-.5.5H3.5a.5.5 0 0 1-.5-.5v-6c0-.28.22-.5.5-.5Z" />
    </svg>
  );
}

function threadConnectionSummary(isLoaded: boolean, connection: RealtimeConnectionSnapshot) {
  if (!isLoaded) {
    return translate("workbench.threadDisconnected");
  }

  switch (connection.status) {
    case 'connected':
      return translate("workbench.realtimeUpdatesConnected");
    case 'reconnecting':
      return translate("workbench.realtimeUpdatesReconnecting");
    case 'offline':
      return translate("workbench.browserOffline");
    case 'checking':
      return translate("workbench.checkingRealtimeConnection");
  }
}

const TERMINAL_OPEN_KEY = 'remote-codex.terminal-open:';

/** Per browser tab, so a reload restores an open terminal without reopening it elsewhere. */
function readTerminalOpen(routeKey: string) {
  try { return sessionStorage.getItem(TERMINAL_OPEN_KEY + routeKey) === '1'; } catch { return false; }
}

function writeTerminalOpen(routeKey: string, open: boolean) {
  try {
    if (open) sessionStorage.setItem(TERMINAL_OPEN_KEY + routeKey, '1');
    else sessionStorage.removeItem(TERMINAL_OPEN_KEY + routeKey);
  } catch { /* Optional preference. */ }
}

export function ThreadDetailPage() {
  const hostScope = useHostScope();
  const { locale: i18nLocale } = useI18n();
  const { id = '' } = useParams();
  const activeThreadIdRef = useRef(id);
  activeThreadIdRef.current = id;
  const location = useLocation();
  const relayRouteDeviceId = relayDeviceIdFromPath(location.pathname);
  const [portsOpen, setPortsOpen] = useState(false);
  useEffect(() => setPortsOpen(false), [relayRouteDeviceId]);
  const routeKey = `${relayRouteDeviceId ?? 'local'}:${id}`;
  const activeRouteRef = useRef(routeKey);
  activeRouteRef.current = routeKey;
  const applyDetailResponseRef = useRef<(detail: ThreadDetailDto) => void>(() => {});
  // Warm a thread on this device before it opens (a pressed row, a dock swipe
  // away), so the switch paints its conversation instead of a spinner.
  const prefetchingRef = useRef(new Set<string>());
  const prefetchThread = useCallback((href: string) => {
    const threadId = threadIdFromPath(href);
    if (!threadId || threadId === id || relayDeviceIdFromPath(href) !== (relayRouteDeviceId ?? null)) return;
    const key = `${relayRouteDeviceId ?? 'local'}:${threadId}`;
    if (recentThreadDetails.has(key) || prefetchingRef.current.has(key)) return;
    prefetchingRef.current.add(key);
    const request = fetchThreadDetail(threadId, { limit: INITIAL_DETAIL_TURN_PAGE_SIZE });
    pendingThreadDetails.set(key, request);
    void request
      .then(detail => { if (!recentThreadDetails.has(key)) rememberThreadDetail(key, detail); })
      .catch(() => { /* Opened normally instead. */ })
      .finally(() => { prefetchingRef.current.delete(key); pendingThreadDetails.delete(key); });
  }, [id, relayRouteDeviceId]);
  const navigate = useNavigate();
  const shellNav = useAppShellNav();
  const plugins = usePlugins();
  const liveOutputBufferRef = useRef('');
  const liveOutputFrameRef = useRef<number | null>(null);
  const supervisorSocketRef = useRef<WebSocket | null>(null);
  const supervisorReconnectTimerRef = useRef<number | null>(null);
  const supervisorHealthInFlightRef = useRef(false);
  const supervisorHealthOkAtRef = useRef<string | null>(null);
  const supervisorHeartbeatRef = useRef(new SupervisorHeartbeat());
  const supervisorBrowserOnlineRef = useRef(
    typeof navigator === 'undefined' ? true : navigator.onLine,
  );
  const supervisorRecoveryPendingRef = useRef(false);
  const shellPanelRef = useRef<ThreadShellPanelHandle | null>(null);
  const composerHostRef = useRef<HTMLDivElement | null>(null);
  const loadRequestIdRef = useRef(0);
  const detailMutationInFlightRef = useRef(0);
  const detailMutationChainRef = useRef<Promise<void>>(Promise.resolve());
  const detailRefreshPendingRef = useRef(false);
  const detailLoadsInFlightRef = useRef(0);
  const detailLoadStartedAtRef = useRef(0);
  // The socket opened while the first summary was loading: re-read it only if
  // the thread turns out to be running (its events may predate the socket).
  const recheckAfterFirstOpenRef = useRef(false);
  const initialLoadRetriesRef = useRef(0);
  const pageContextRequestIdRef = useRef(0);
  const pageContextProviderRef = useRef<ThreadDto['provider'] | null>(null);
  const terminalTurnPendingRef = useRef<string | null>(null);
  const detailRef = useRef<ThreadDetailDto | null>(null);
  const promptSubmissionOwnerRef = useRef({ routeKey, inFlight: false });
  if (promptSubmissionOwnerRef.current.routeKey !== routeKey)
    promptSubmissionOwnerRef.current = { routeKey, inFlight: false };
  const promptSubmissionOwner = promptSubmissionOwnerRef.current;
  const interruptingRef = useRef(false);
  const pendingThreadSettingsRef = useRef<PendingThreadSettings | null>(null);
  const resolvedRequestIdsRef = useRef<Set<string>>(new Set());
  const [detail, setDetail] = useScopedState<ThreadDetailDto | null>(routeKey, null);
  const [threads, setThreads] = useScopedState<ThreadDto[]>(relayRouteDeviceId ?? 'local', []);
  detailRef.current = detail;
  const [modelOptions, setModelOptions] = useState<ModelOptionDto[]>([]);
  const [agentOptions, setAgentOptions] = useState<ModelOptionDto[]>([]);
  const [status, setStatus] = useState<AgentRuntimeStatusDto | null>(null);
  const [backendCapabilities, setBackendCapabilities] = useState<AgentProviderCapabilitiesDto | null>(null);
  const subscriptionUsage = useSubscriptionUsage({
    deviceId: relayRouteDeviceId,
    threadId: id,
    provider: hostScope ? undefined : detail?.thread.provider,
    agentId: detail?.thread.agentId,
  });
  const [backendManagementSchema, setBackendManagementSchema] =
    useState<AgentBackendManagementSchemaDto | null>(null);
  const [liveOutput, setLiveOutput] = useState('');
  const [backendProgress, setBackendProgress] = useState<{ threadId: string; turnId: string; receivedAt: string } | null>(null);
  const [livePlan, setLivePlan] = useState<{
    turnId: string;
    explanation: string | null;
    plan: Array<{ step: string; status: string }>;
  } | null>(null);
  const [liveItems, setLiveItems] = useState<
    NonNullable<ThreadDetailDto['liveItems']> | null
  >(null);
  const liveItemsRef = useRef<
    NonNullable<ThreadDetailDto['liveItems']> | null
  >(null);
  const [followTail, setFollowTail] = useState(true);
  const [scrollRequestKey, setScrollRequestKey] = useState(0);
  const [previousTurnScrollRequestKey, setPreviousTurnScrollRequestKey] = useState(0);
  const [nextTurnScrollRequestKey, setNextTurnScrollRequestKey] = useState(0);
  const [canJumpToPreviousTurn, setCanJumpToPreviousTurn] = useState(false);
  const [canJumpToNextTurn, setCanJumpToNextTurn] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const [mutationBusy, setBusy] = useScopedState(routeKey, false);
  const autoConnectionAttempt = useRef<{ key: string; promise: Promise<ThreadDetailDto> | null } | null>(null);
  const [autoConnectingRoute, setAutoConnectingRoute] = useState<string | null>(null);
  const busy = mutationBusy || autoConnectingRoute === routeKey;
  const [activeView, setActiveView] = useState<'chat' | 'shell'>('chat');
  const searchLabels = useSearchMessages();
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchTarget, setSearchTarget] = useState<{ turnId: string; itemId: string; key: number; query?: string }>();
  // Global search opens a message through a shareable, device-scoped deep link.
  // Load only the destination turn, including turns outside the summary page.
  useEffect(() => {
    if (!id || detail?.thread.id !== id) return;
    const params = new URLSearchParams(location.search);
    const turnId = params.get('searchTurn'), itemId = params.get('searchItem'), query = params.get('searchQuery');
    if (!turnId || !itemId) return;
    let cancelled = false;
    void fetchThreadTurnDetail(id, turnId).then(turn => {
      if (cancelled) return;
      setDetail(current => current?.thread.id === id ? {
        ...current,
        turns: prependTurns(current.turns.map(existing => existing.id === turnId ? turn : existing), [turn]),
      } : current);
      setSearchTarget({ turnId, itemId, key: Date.now(), ...(query ? { query } : {}) });
    }).catch(caught => { if (!cancelled) setError(caught instanceof Error ? caught.message : searchLabels.openFailed); });
    return () => { cancelled = true; };
  }, [id, detail?.thread.id, location.search]);
  const [actionMode, setActionMode] = useState<'share' | 'link' | 'html'>('share');
  const [workspaceFocusPathRequest, setWorkspaceFocusPathRequest] =
    useState<WorkspaceFocusPathRequest | null>(null);
  const terminalPluginEnabled = !hostScope && plugins.getThreadPanels().some(
    (panel) => panel.kind === 'terminal',
  );
  useEffect(() => { if (!terminalPluginEnabled) setActiveView('chat'); }, [terminalPluginEnabled]);

  useEffect(() => {
    liveItemsRef.current = liveItems;
  }, [liveItems]);

  const localShellAdapter = useMemo(
    () => ({
      fetchState: fetchThreadShellState,
      createShell: createThreadShell,
      terminateShell,
      updateShell,
      connectSocket: connectShellSocket,
    }),
    [],
  );
  const getThreadHref = useCallback(
    (threadId: string) => appUrl(currentThreadHref(threadId)),
    [],
  );
  const openThread = useCallback(
    (threadId: string) => {
      navigate(currentThreadHref(threadId));
    },
    [navigate],
  );
  const getNewThreadHref = useCallback(
    (workspaceId?: string | null) => appUrl(currentNewThreadHref(workspaceId)),
    [],
  );
  const [harnessSettingsTarget, setHarnessSettingsTarget] = useState<{
    pane: 'primary' | 'reference'; deviceId: string; threadId: string; composerFocusOwner?: string;
  } | null>(null);
  const renderNewThreadDialogContent = useCallback(
    ({
      close,
      closeNavigation,
      currentWorkspaceId,
    }: {
      close: () => void;
      closeNavigation: () => void;
      currentWorkspaceId?: string | null;
    }) => (
      <ThreadCreateForm
        variant="dialog"
        initialWorkspaceId={currentWorkspaceId}
        onCancel={close}
        onCreated={(thread) => {
          close();
          closeNavigation();
          navigate(currentThreadHref(thread.id));
        }}
      />
    ),
    [navigate],
  );
  const getThreadImageAssetUrl = useCallback(
    ({ threadId, path, variant }: { threadId: string; path: string; variant?: 'thumbnail' }) =>
      buildThreadImageAssetUrl(threadId, { path, variant }),
    [],
  );
  const [nativeResult, setNativeResult] = useScopedState<{ title: string; text: string } | null>(routeKey, null);
  const [shellControlState, setShellControlState] =
    useState<ThreadShellControlState | null>(null);
  const [pendingShellConnectionToggle, setPendingShellConnectionToggle] =
    useState(false);
  const [settingsBusy, setSettingsBusy] = useScopedState(routeKey, false);
  const [compactBusy, setCompactBusy] = useScopedState(routeKey, false);
  const [respondingRequestId, setRespondingRequestId] = useScopedState<string | null>(routeKey, null);
  const [metaSessionCopyState, setMetaSessionCopyState] =
    useState<'idle' | 'copied' | 'failed'>('idle');
  const [realtimeConnection, setRealtimeConnection] =
    useState<RealtimeConnectionSnapshot>({
      status: supervisorBrowserOnlineRef.current ? 'checking' : 'offline',
      browserOnline: supervisorBrowserOnlineRef.current,
      healthOk: false,
      socketOpen: false,
      lastHealthyAt: null,
    });
  const [optimisticTurn, setOptimisticTurn] = useScopedState<OptimisticTurnState | null>(routeKey, null);
  const [optimisticSteers, setOptimisticSteers] = useScopedState<OptimisticSteerState[]>(routeKey, []);
  useEffect(() => {
    const previews = optimisticTurn?.attachmentPreviews ?? [];
    return () => {
      revokeOptimisticAttachmentPreviews(previews);
    };
  }, [optimisticTurn]);
  const [deletingThread, setDeletingThread] = useState<ThreadDto | null>(null);
  const [deletingThreadBusy, setDeletingThreadBusy] = useState(false);
  const [error, setError] = useScopedState<string | null>(routeKey, null);
  const mcpProviderConfigFileName =
    backendManagementSchema?.hostConfigFiles.find((file) => file.roles?.includes('mcp'))
      ?.name ?? null;
  const {
    exportBusy,
    exportDialogOpen,
    exportTurnsState,
    forkTurnOptionsState,
    goalState,
    handleCreateHook,
    handleExportTranscript,
    handleForkLatest,
    handleForkTurn,
    handleOpenForkTurns,
    handleOpenGoal,
    handleOpenHooks,
    handleOpenMcp,
    handleOpenSkills,
    handleTrustHook,
    handleUntrustHook,
    handleUpdateGoal,
    handleUpdateHook,
    hooksState,
    loadExportTurns,
    mcpState,
    setExportDialogOpen,
    setGoalState,
    skillsState,
  } = useThreadAuxiliaryActions({
    detailRef,
    id,
    navigate,
    setDetail,
    setError,
    setThreads,
  });
  const shareDialogContexts = useRef(new Map<string, SharingInput>());
  const [shareBusy, setShareBusy] = useState(false);
  const [threadShareState, setThreadShareState] = useState<{
    status: 'idle' | 'loading' | 'ready' | 'failed';
    shares: ThreadShareSummary[];
    error: string | null;
  }>({
    status: 'idle',
    shares: [],
    error: null,
  });
  const [relayAccessState, setRelayAccessState] = useState<{
    status: 'idle' | 'loading' | 'ready' | 'failed';
    access: RelayEffectiveAccessDto | null;
    error: string | null;
  }>({
    status: 'idle',
    access: null,
    error: null,
  });
  const workbenchNavigation = useWorkbenchNavigation(detail, threads, relayRouteDeviceId, id, { sharesDeviceThreads: true });
  const relayDeviceRouteActive =
    relayModeActive() && Boolean(relayRouteDeviceId);
  const relayAccess = relayAccessState.access;
  const relayThreadIsOwner =
    !hostScope && (!relayDeviceRouteActive || relayAccess?.kind === 'owner');
  const relayThreadCanControl =
    Boolean(hostScope) || relayThreadIsOwner ||
    (relayAccess?.kind === 'shared' && relayAccess.threadAccess === 'control');
  const relayThreadCanShare =
    relayDeviceRouteActive && relayAccess?.kind === 'owner';
  const currentWorkspaceId =
    detail?.workspace.id ?? detail?.thread.workspaceId ?? null;
  const [presentationAccount, setPresentationAccount] = useScopedState<string | null>(relayRouteDeviceId ?? 'local', relayDeviceRouteActive ? null : 'local');
  const [presentationDeviceName, setPresentationDeviceName] = useScopedState<string | null>(relayRouteDeviceId ?? 'local', null);
  useEffect(() => {
    if (!relayDeviceRouteActive) return;
    let alive = true;
    void fetchRelaySession().then(session => { if (alive) setPresentationAccount(session.user?.id ?? null); }).catch(() => {});
    void fetchRelayPortal().then(portal => { if (alive) setPresentationDeviceName(portal.devices.find(device => device.id === relayRouteDeviceId)?.name ?? null); }).catch(() => {});
    return () => { alive = false; };
  }, [relayRouteDeviceId, relayDeviceRouteActive, setPresentationAccount, setPresentationDeviceName]);
  // Keep the workspace profile through a primary swap while the route is loading.
  const presentationWorkspace = useRef<{ device: string | null; id: string | null }>({ device: relayRouteDeviceId, id: currentWorkspaceId });
  if (presentationWorkspace.current.device !== relayRouteDeviceId || currentWorkspaceId) presentationWorkspace.current = { device: relayRouteDeviceId, id: currentWorkspaceId };
  const browserStateScope = useBrowserStateKey('remote-codex.presentation.v1');
  const presentationScope = presentationAccount && presentationWorkspace.current.id ? `${browserStateScope}:${JSON.stringify([window.location.origin, presentationAccount, relayRouteDeviceId ?? 'local', presentationWorkspace.current.id])}` : null;
  const workbenchPresentation = useWorkbenchPresentation(presentationScope, routeKey);
  const referenceDevice = workbenchPresentation.value.referenceDeviceId ?? relayRouteDeviceId ?? 'local';
  const referenceAllowed = !hostScope || !workbenchPresentation.value.referenceId || (
    referenceDevice === 'local' && threads.some(thread =>
      thread.id === workbenchPresentation.value.referenceId)
  );
  const referenceId = !referenceAllowed || (
    workbenchPresentation.value.referenceId === id && referenceDevice === (relayRouteDeviceId ?? 'local')
  ) ? null : workbenchPresentation.value.referenceId;
  const [focusedPane, setFocusedPane] = useScopedState<'primary' | 'reference'>(`${routeKey}:${referenceDevice}:${referenceId ?? ''}`, 'primary');
  const [storedTerminalOpen, setTerminalOpenState] = useScopedState(routeKey, readTerminalOpen(routeKey));
  const terminalOpen = !hostScope && storedTerminalOpen;
  // One tools drawer: the terminal or a plugin panel (DeepSeek Harness).
  const [dshOpen, setDshOpen] = useScopedState(routeKey, false);
  // Restored after a reload; an explicit open also focuses and may create a terminal.
  const setTerminalOpen = useCallback((action: SetStateAction<boolean>) => {
    setTerminalOpenState(current => {
      const next = typeof action === 'function' ? action(current) : action;
      writeTerminalOpen(routeKey, next);
      return next;
    });
  }, [routeKey, setTerminalOpenState]);
  const [terminalOpenRequest, setTerminalOpenRequest] = useState<TerminalOpenRequest | null>(null);
  const [chatDraft, setChatDraft, referenceDraft, setReferenceDraft] = useThreadDrafts(routeKey, `${referenceDevice}:${referenceId ?? ''}`);
  const referenceShown = Boolean(referenceId) && workbenchPresentation.value.mode === 'thread';
  const referenceController = useWorkbenchReference(referenceDevice, referenceId, relayRouteDeviceId ?? 'local', referenceShown);
  useEffect(() => {
    if (!hostScope || !referenceId || !referenceController.unavailable) return;
    void workbenchNavigation.onThreadRemoved(`local:${referenceId}`);
    setThreads(previous => previous.filter(thread => thread.id !== referenceId));
    workbenchPresentation.update({ referenceId: null, mode: 'focus' });
  }, [hostScope, referenceId, referenceController.unavailable, workbenchPresentation.update]);
  // The Supervisor keeps displayed harnesses loaded: the primary thread and a
  // split showing a conversation on the same device (the split reports its own
  // device). View-only shares never count.
  const threadView = useThreadView(supervisorSocketRef, [
    ...(relayThreadCanControl ? [id] : []),
    ...(referenceShown && referenceController.canControl && referenceDevice === (relayRouteDeviceId ?? 'local') ? [referenceId!] : []),
  ]);
  const referenceEventRef = useRef(referenceController.onEvent);
  referenceEventRef.current = referenceController.onEvent;
  const onCompareThread = useCallback((threadId: string) => {
    if (threadId === activeThreadIdRef.current) return;
    if (!confirmWorkspaceDocumentLeave()) return;
    workbenchPresentation.update({ referenceId: threadId, referenceDeviceId: null, mode: 'thread' });
    setFocusedPane('primary');
  }, [workbenchPresentation.update, setFocusedPane]);
  const makeReferencePrimary = () => {
    if (!referenceId) return;
    // The file guard can cancel navigation. Keep reference membership unchanged
    // until that decision succeeds, or cancellation would hide the chosen peer.
    if (!confirmWorkspaceDocumentLeave()) return;
    workbenchPresentation.update({ referenceId: id, referenceDeviceId: relayRouteDeviceId, mode: 'thread' });
    navigate(threadHref(referenceId, referenceDevice === 'local' ? null : referenceDevice), {
      state: { workbenchSwap: { targetDevice: referenceDevice, targetThread: referenceId, referenceId: id, referenceDeviceId: relayRouteDeviceId } },
    });
  };
  const effectiveWorkspaceAccess: 'none' | 'read' | 'write' =
    hostScope ? 'read' : !relayDeviceRouteActive
      ? 'write'
      : relayAccess?.kind === 'owner'
        ? 'write'
        : relayAccess?.kind === 'shared' &&
            // Whole-device grants cover every workspace and have no workspaceId.
            // Resource-scoped grants must still match this workspace explicitly.
            ((relayAccess.scope === 'device' && relayAccess.workspaceScope !== 'selected') ||
              (relayAccess.workspaceId && relayAccess.workspaceId === currentWorkspaceId))
          ? relayAccess.workspaceAccess
          : 'none';
  const appliedSwap = useRef<unknown>(null);
  useEffect(() => {
    const state = location.state as {workbenchSwap?: {targetDevice:string; targetThread:string; referenceId:string; referenceDeviceId:string|null}} | null;
    const swap = state?.workbenchSwap;
    if (!swap || appliedSwap.current === swap || swap.targetDevice !== (relayRouteDeviceId ?? 'local') || swap.targetThread !== id) return;
    appliedSwap.current = swap;
    // Carry the old primary across workspace/device scopes through route state;
    // the presentation hook retains this action while identity is loading.
    workbenchPresentation.update({referenceId:swap.referenceId, referenceDeviceId:swap.referenceDeviceId, mode:'thread'});
    navigate(`${location.pathname}${location.search}${location.hash}`, {replace:true, state:null});
  }, [location.state, location.pathname, location.search, location.hash, relayRouteDeviceId, id, navigate, workbenchPresentation.update]);
  const toolsUseReference = focusedPane === 'reference' && Boolean(referenceId) && workbenchPresentation.value.mode !== 'focus';
  // The files' prompt edits the draft of the conversation it sends to, so text
  // typed there is in that composer too; an accepted send clears it.
  const toolsDraft = toolsUseReference ? referenceDraft : chatDraft;
  const setToolsDraft = toolsUseReference ? setReferenceDraft : setChatDraft;
  // The file the files pane shows; a question asked there names it.
  const [toolsFile, setToolsFile] = useState<string | null>(null);
  const sendFromFiles = async (prompt: string) => {
    const { attachments } = toolsDraft;
    const input = { prompt, ...(attachments.length ? { attachments } : {}) };
    if (!toolsUseReference) return handlePrompt(input);
    const sent = await referenceController.send(input);
    if (sent) setReferenceDraft({ prompt: '', attachments: [] });
    return sent;
  };
  const toolsDetail = toolsUseReference ? referenceController.detail : detail;
  const toolsDevice = toolsUseReference ? referenceDevice : (relayRouteDeviceId ?? 'local');
  const toolsAccess = toolsUseReference ? referenceController.access : relayAccess;
  const toolsRelay = toolsDevice !== 'local' && relayModeActive();
  const toolsWorkspaceAccess = !toolsRelay || toolsAccess?.kind === 'owner' ? 'write'
    : toolsAccess?.kind === 'shared' && ((toolsAccess.scope === 'device' && toolsAccess.workspaceScope !== 'selected') || toolsAccess.workspaceId === toolsDetail?.workspace.id)
      ? toolsAccess.workspaceAccess : 'none';
  const toolsCanControl = toolsUseReference ? referenceController.canControl : relayThreadCanControl;
  const toolsTargetLabel = `${toolsDevice === (relayRouteDeviceId ?? 'local') ? (presentationDeviceName ?? translate('workbench.localDeviceName')) : toolsDevice.slice(0,8)} · ${toolsDetail?.workspace.label ?? ''} · ${toolsDetail?.thread.title ?? ''}`;
  const focusPane = useCallback((pane: 'primary' | 'reference') => {
    if (pane === focusedPane) return true;
    if (pane === 'reference' && !referenceId) return false;
    const target = pane === 'reference' ? referenceController.detail : detail;
    const device = pane === 'reference' ? referenceDevice : (relayRouteDeviceId ?? 'local');
    if ((device !== toolsDevice || target?.workspace.id !== toolsDetail?.workspace.id) && !confirmWorkspaceDocumentLeave()) return false;
    setFocusedPane(pane);
    setWorkspaceFocusPathRequest(null);
    return true;
  }, [focusedPane, referenceId, referenceController.detail, detail, referenceDevice, relayRouteDeviceId, toolsDevice, toolsDetail?.workspace.id, setFocusedPane]);
  const changePresentation = useCallback((patch: Partial<typeof workbenchPresentation.value>) => {
    if (patch.mode === 'focus' || patch.mode === 'collaboration') {
      if (toolsUseReference && (toolsDevice !== (relayRouteDeviceId ?? 'local') || toolsDetail?.workspace.id !== detail?.workspace.id) && !confirmWorkspaceDocumentLeave()) return false;
      setFocusedPane('primary');
    }
    workbenchPresentation.update(patch);
    return true;
  }, [workbenchPresentation.update, toolsUseReference, toolsDevice, toolsDetail?.workspace.id, relayRouteDeviceId, detail?.workspace.id, setFocusedPane]);
  const selectSplitThread = useCallback((selection: {deviceId: string | null; workspaceId: string; threadId: string; title: string}) => {
    if (toolsUseReference && (toolsDevice !== (relayRouteDeviceId ?? 'local') || toolsDetail?.workspace.id !== detail?.workspace.id) && !confirmWorkspaceDocumentLeave()) return;
    workbenchPresentation.update({ referenceId: selection.threadId, referenceDeviceId: selection.deviceId, mode: 'thread' });
    setFocusedPane('primary');
  }, [toolsUseReference, toolsDevice, toolsDetail?.workspace.id, relayRouteDeviceId, detail?.workspace.id, workbenchPresentation.update, setFocusedPane]);
  // Closing the drawer closes whichever panel it shows.
  const closeTerminal = useCallback(() => { setTerminalOpen(false); setDshOpen(false); }, [setTerminalOpen, setDshOpen]);
  const toolsTargetKey = toolsDetail ? terminalTargetKey(toolsDevice, toolsDetail.thread.id) : null;
  const toggleTerminal = useCallback(() => {
    if (!terminalOpen && toolsTargetKey) {
      setTerminalOpenRequest(current => ({ token: (current?.token ?? 0) + 1, target: toolsTargetKey }));
    }
    if (!terminalOpen) setDshOpen(false);
    setTerminalOpen(!terminalOpen);
  }, [setTerminalOpen, setDshOpen, terminalOpen, toolsTargetKey]);
  const loadToolsCapabilities = useCallback(() => fetchThreadCapabilitySnapshot(toolsDetail!.thread.id, toolsDevice), [toolsDetail?.thread.id, toolsDevice]);
  const dshPanelAvailable = !hostScope && toolsDetail?.thread.agentId === 'deepseek'
    && plugins.getThreadPanels().some(panel => panel.kind === DEEPSEEK_HARNESS_PANEL_KIND);
  const dshPanelOpen = dshOpen && dshPanelAvailable;
  const toggleDshPanel = useCallback(() => {
    if (!dshPanelOpen) setTerminalOpen(false);
    setDshOpen(!dshPanelOpen);
  }, [dshPanelOpen, setDshOpen, setTerminalOpen]);
  // Harness panel actions edit the device's DSH profile: owner-only on Relay.
  const referenceHarnessOwner = !hostScope && referenceController.canControl
    && (referenceController.access?.kind === 'owner' || referenceDevice === 'local' || !relayModeActive());
  const toolsHarnessOwner = toolsUseReference ? referenceHarnessOwner : relayThreadIsOwner;
  // Harness panel actions stay on the device that owns the thread.
  const harnessPanel = useCallback((threadId: string, deviceId: string | null | undefined) => ({
    runHarnessAction: (action: DshPanelAction) => postThreadHarnessAction<DshPanelResult>(threadId, action, deviceId),
    dshConsoleUrl: (target: DshConsoleTarget) => dshConsoleUrl(threadId, target, deviceId),
  }), []);
  const dshPanelProps = useMemo(() => toolsDetail ? harnessPanel(toolsDetail.thread.id, toolsDevice) : null, [harnessPanel, toolsDetail?.thread.id, toolsDevice]);
  const loadDialogCapabilities = useCallback(() => fetchThreadCapabilitySnapshot(harnessSettingsTarget!.threadId, harnessSettingsTarget!.deviceId), [harnessSettingsTarget?.threadId, harnessSettingsTarget?.deviceId]);
  const referencePluginAdapter = useMemo(
    () => threadPluginAdapter(referenceDevice === 'local' ? null : referenceDevice),
    [referenceDevice],
  );
  const openReferenceThread = useCallback((threadId: string) => navigate(threadHref(threadId, referenceDevice === 'local' ? null : referenceDevice)), [navigate, referenceDevice]);
  const loadThreadShares = useCallback(async () => {
    const currentDetail = detailRef.current;
    const deviceId = currentRelayDeviceIdFromPath();
    if (!relayModeActive() || !currentDetail || !deviceId) {
      setThreadShareState({
        status: 'ready',
        shares: [],
        error: null,
      });
      return;
    }

    setThreadShareState((current) => ({
      ...current,
      status: 'loading',
      error: null,
    }));
    try {
      const portal = await fetchRelayPortal();
      const ownedShares = portal.sharedByMe.filter(
          (share) =>
            share.deviceId === deviceId &&
            share.threadId === currentDetail.thread.id,
        );
      const threadTitle = currentDetail.thread.title;
      const workspaceLabel = currentDetail.workspace.label;
      await Promise.all(
        ownedShares
          .filter(
            (share) =>
              share.threadTitle !== threadTitle ||
              (share.workspaceId && share.workspaceLabel !== workspaceLabel),
          )
          .map((share) =>
            updateRelayShare(share.id, {
              threadTitle,
              ...(share.workspaceId ? { workspaceLabel } : {}),
            }),
          ),
      );
      shareDialogContexts.current = new Map([
        ...ownedShares.map(share => [share.id, sharingEditInput('share', share)] as const),
        ...(portal.grantsByMe ?? []).filter(grant => grant.deviceId === deviceId && grant.scope === 'device')
          .map(grant => [`grant:${grant.id}`, sharingEditInput('grant', grant)] as const),
      ]);
      const shares: ThreadShareSummary[] = ownedShares
        .map((share) => ({
          id: share.id,
          trustContent: <SignedShareDetails kind="share" row={share} />,
          targetUsername: share.targetUsername,
          label: share.label,
          threadAccess: share.threadAccess,
          workspaceAccess: share.workspaceAccess,
          createdAt: share.createdAt,
        }));
      shares.push(...(portal.grantsByMe ?? []).filter(grant => grant.deviceId === deviceId && grant.scope === 'device').map(grant => ({
        trustContent: <SignedShareDetails kind="grant" row={grant} />,
        id: `grant:${grant.id}`, scope: 'device' as const, targetUsername: grant.targetUsername, label: grant.label,
        threadAccess: grant.threadAccess, workspaceAccess: grant.workspaceAccess, createdAt: grant.createdAt,
      })));
      setThreadShareState({
        status: 'ready',
        shares,
        error: null,
      });
    } catch (caught) {
      setThreadShareState((current) => ({
        ...current,
        status: 'failed',
        error: actionErrorMessage(caught, translate("workbench.unableToLoadActiveShares")),
      }));
    }
  }, [detailRef]);
  const handleCreateThreadShare = useCallback(
    async (input: CreateThreadShareInput) => {
      const currentDetail = detailRef.current;
      const deviceId = currentRelayDeviceIdFromPath();
      if (!relayModeActive() || !currentDetail || !deviceId) {
        setThreadShareState((current) => ({
          ...current,
          status: 'failed',
          error: translate("workbench.relaySharingIsOnlyAvailableFromA"),
        }));
        return;
      }

      const workspaceId =
        input.workspaceAccess === 'none'
          ? null
          : currentDetail.workspace.id ??
            currentDetail.thread.workspaceId ??
            null;
      if (input.workspaceAccess !== 'none' && !workspaceId) {
        setThreadShareState((current) => ({
          ...current,
          status: 'failed',
          error: translate("workbench.thisThreadIsNotAttachedToA"),
        }));
        return;
      }

      setShareBusy(true);
      setThreadShareState((current) => ({
        ...current,
        error: null,
      }));
      try {
        if (input.scope === 'device') await createRelayGrant({
          targetIdentifier: input.targetIdentifier, deviceId, scope: 'device', workspaceScope: 'all', workspaceIds: [],
          label: input.label ?? null, threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess, canCreateThreads: false,
        });
        else await createRelayShare({
          targetIdentifier: input.targetIdentifier,
          deviceId,
          threadId: currentDetail.thread.id,
          threadTitle: currentDetail.thread.title,
          workspaceId,
          workspaceLabel: workspaceId ? currentDetail.workspace.label : null,
          label: input.label ?? null,
          threadAccess: input.threadAccess,
          workspaceAccess: input.workspaceAccess,
        });
        await loadThreadShares();
      } catch (caught) {
        const message = actionErrorMessage(caught, translate("workbench.unableToCreateShare"));
        setThreadShareState((current) => ({
          ...current,
          status: 'failed',
          error: message,
        }));
        setError(message);
        throw caught;
      } finally {
        setShareBusy(false);
      }
    },
    [detailRef, loadThreadShares],
  );
  const handleUpdateThreadShare = useCallback(async (shareId: string, input: CreateThreadShareInput) => {
    const currentDetail = detailRef.current;
    if (!currentDetail) return;
    setShareBusy(true);
    try {
      const context = shareDialogContexts.current.get(shareId);
      if (!context) throw new Error(translate('sharing.dialogIncomplete'));
      if (shareId.startsWith('grant:')) await updateRelayGrant(shareId.slice(6), {
        threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess, label: input.label ?? null,
      }, context);
      else await updateRelayShare(shareId, {
        threadAccess: input.threadAccess, workspaceAccess: input.workspaceAccess,
        workspaceId: currentDetail.workspace.id,
        workspaceLabel: currentDetail.workspace.label,
        label: input.label ?? null,
      }, context);
      await loadThreadShares();
    } catch (caught) {
      setThreadShareState(current => ({...current, status:'failed', error:actionErrorMessage(caught, translate("workbench.unableToUpdatePermissions"))}));
      throw caught;
    } finally {setShareBusy(false);}
  }, [detailRef, loadThreadShares]);
  const handleRevokeThreadShare = useCallback(async (shareId: string) => {
    setShareBusy(true);
    setThreadShareState((current) => ({
      ...current,
      error: null,
    }));
    try {
      if (shareId.startsWith('grant:')) await revokeRelayGrant(shareId.slice(6));
      else await revokeRelayShare(shareId);
      await loadThreadShares();
    } catch (caught) {
      const message = actionErrorMessage(caught, translate("workbench.unableToRevokeShare"));
      setThreadShareState((current) => ({
        ...current,
        status: 'failed',
        error: message,
      }));
      setError(message);
    } finally {
      setShareBusy(false);
    }
  }, [loadThreadShares]);
  useEffect(() => {
    if (detail?.thread.id && relayThreadCanShare) {
      void loadThreadShares();
    }
  }, [detail?.thread.id, relayThreadCanShare, exportDialogOpen, loadThreadShares]);
  useEffect(() => {
    const currentDetail = detailRef.current;
    const deviceId = currentRelayDeviceIdFromPath();
    if (!relayModeActive() || !currentDetail || !deviceId) {
      setRelayAccessState({
        status: 'idle',
        access: null,
        error: null,
      });
      return;
    }

    let cancelled = false;
    setRelayAccessState((current) => ({
      ...current,
      status: 'loading',
      error: null,
    }));
    fetchRelayAccess({
      deviceId,
      threadId: currentDetail.thread.id,
    })
      .then((access) => {
        if (cancelled) {
          return;
        }
        setRelayAccessState({
          status: 'ready',
          access,
          error: null,
        });
      })
      .catch((caught) => {
        if (cancelled) {
          return;
        }
        setRelayAccessState({
          status: 'failed',
          access: null,
          error: actionErrorMessage(caught, translate("workbench.unableToVerifyRelayPermissions")),
        });
      });

    return () => {
      cancelled = true;
    };
  }, [detail?.thread.id, detail?.workspace.id, detailRef]);
  // One list for the page, its tabs and the workbench navigation. Other
  // threads' socket events refresh it; the fallback covers renames and deletes.
  const threadList = useThreadListPolling({
    enabled: Boolean(id),
    setThreads,
    includeAgentThreads: true,
    live: realtimeConnection.socketOpen,
  });
  const threadListRef = useRef(threadList);
  threadListRef.current = threadList;

  const flushBufferedLiveOutput = useCallback(() => {
    const buffered = liveOutputBufferRef.current;
    liveOutputBufferRef.current = '';
    liveOutputFrameRef.current = null;

    if (!buffered) {
      return;
    }

    startTransition(() => {
      setLiveOutput((current) => current + buffered);
    });
  }, []);

  const queueLiveOutputDelta = useCallback(
    (delta: string) => {
      liveOutputBufferRef.current += delta;
      if (liveOutputFrameRef.current !== null) {
        return;
      }

      liveOutputFrameRef.current = window.requestAnimationFrame(() => {
        flushBufferedLiveOutput();
      });
    },
    [flushBufferedLiveOutput],
  );

  const upsertLiveTimelineItem = useCallback(
    (turnId: string, item: ThreadHistoryItemDto) => {
      setLiveItems((current) => {
        const currentItems =
          current?.turnId === turnId ? current.items : [];
        const existingIndex = currentItems.findIndex((entry) => entry.id === item.id);
        const nextItem = mergeLiveHistoryItem(currentItems[existingIndex], item);
        const nextItems =
          existingIndex >= 0
            ? currentItems.map((entry, index) => (index === existingIndex ? nextItem : entry))
            : [...currentItems, nextItem];
        return {
          turnId,
          items: nextItems,
          updatedAt: new Date().toISOString(),
        };
      });
    },
    [],
  );

  const appendLiveAgentDelta = useCallback(
    (
      turnId: string,
      itemId: string,
      delta: string,
      sequence: number | null,
      createdAt?: string | null,
      text?: string | null,
    ) => {
      setLiveItems((current) => appendLiveAgentDeltaToItems(
        current,
        detailRef.current?.turns ?? [],
        { turnId, itemId, delta, sequence, createdAt: createdAt ?? null, text: text ?? null },
      ));
    },
    [],
  );

  const clearBufferedLiveOutput = useCallback(() => {
    liveOutputBufferRef.current = '';
    if (liveOutputFrameRef.current !== null) {
      window.cancelAnimationFrame(liveOutputFrameRef.current);
      liveOutputFrameRef.current = null;
    }
  }, []);

  const applyDetailResponse = useCallback(
    (detailResponse: ThreadDetailDto) => {
      if (activeRouteRef.current !== routeKey || detailResponse.thread.id !== id) return;
      rememberThreadDetail(routeKey, detailResponse);
      const pendingThreadSettings = pendingThreadSettingsRef.current;
      const nextDetail =
        pendingThreadSettings && Object.keys(pendingThreadSettings).length > 0
          ? {
              ...detailResponse,
              thread: {
                ...detailResponse.thread,
                ...pendingThreadSettings,
              },
            }
          : detailResponse;
      const nextDetailWithLiveTimestamps = {
        ...nextDetail,
        totalTurnCount: nextDetail.totalTurnCount ?? detailRef.current?.totalTurnCount ?? nextDetail.turns.length,
        turns: applyLiveItemTimestampsToTurns(nextDetail.turns, liveItemsRef.current),
      };
      const previousDetail = detailRef.current;
      setLivePlan(nextDetailWithLiveTimestamps.livePlan ?? null);
      const mergedTurns = appendLatestTurns(
        previousDetail?.turns ?? [],
        nextDetailWithLiveTimestamps.turns,
        nextDetailWithLiveTimestamps.thread.activeTurnId,
      );
      detailRef.current = {
        ...nextDetailWithLiveTimestamps,
        turns: mergedTurns,
      };
      setLiveItems((current) =>
        reconcileLiveItemsWithDetail(
          current,
          nextDetailWithLiveTimestamps.liveItems ?? null,
          mergedTurns,
        ),
      );
      setGoalState((current) =>
        current.status === 'idle'
          ? current
          : {
              ...current,
              data: nextDetailWithLiveTimestamps.goal ?? null,
            },
      );
      const threadHasEnded =
        nextDetailWithLiveTimestamps.thread.activeTurnId === null &&
        nextDetailWithLiveTimestamps.thread.status !== 'running';

      setDetail((current) =>
        current && !nextDetailWithLiveTimestamps.goalHistory
          ? {
              ...nextDetailWithLiveTimestamps,
              turns: appendLatestTurns(
                current.turns,
                nextDetailWithLiveTimestamps.turns,
                nextDetailWithLiveTimestamps.thread.activeTurnId,
              ),
              pendingRequests: mergePendingRequests(
                current.pendingRequests,
                nextDetailWithLiveTimestamps.pendingRequests,
                resolvedRequestIdsRef.current,
              ),
              ...(current.goalHistory ? { goalHistory: current.goalHistory } : {}),
            }
          : current
            ? {
                ...nextDetailWithLiveTimestamps,
                turns: appendLatestTurns(
                  current.turns,
                  nextDetailWithLiveTimestamps.turns,
                  nextDetailWithLiveTimestamps.thread.activeTurnId,
                ),
                pendingRequests: mergePendingRequests(
                  current.pendingRequests,
                  nextDetailWithLiveTimestamps.pendingRequests,
                  resolvedRequestIdsRef.current,
                ),
              }
            : {
                ...nextDetailWithLiveTimestamps,
                turns: mergedTurns,
              },
      );
      setThreads((current) =>
        mergeThreadIntoList(current, nextDetailWithLiveTimestamps.thread),
      );
      const nextTurnsById = new Map(
        nextDetailWithLiveTimestamps.turns.map((turn) => [turn.id, turn] as const),
      );
      const pendingSteerRequestIds = new Set(
        (nextDetailWithLiveTimestamps.pendingSteers ?? [])
          .map((steer) => steer.clientRequestId)
          .filter((value): value is string => Boolean(value)),
      );
      setOptimisticSteers((current) =>
        current.filter((steer) => {
          if (pendingSteerRequestIds.has(steer.clientRequestId)) {
            return false;
          }

          const targetTurn = nextTurnsById.get(steer.turnId);
          if (!targetTurn) {
            return false;
          }

          if (turnHasUserMessage(targetTurn, steer.prompt)) {
            return false;
          }

          if (
            nextDetailWithLiveTimestamps.thread.activeTurnId !== steer.turnId &&
            targetTurn.status !== 'inProgress'
          ) {
            return false;
          }

          return true;
        }),
      );
      setOptimisticTurn((current) => {
        if (!current) {
          return current;
        }
        if (
          current.id.startsWith('optimistic-goal-') &&
          nextDetailWithLiveTimestamps.activityNotes?.some(
            (note) => note.kind === 'goal' && note.text === current.prompt,
          )
        ) {
          return null;
        }

        const resolvedTurnId = current.serverTurnId ?? current.id;
        const hasMaterializedTurn = nextDetailWithLiveTimestamps.turns.some(
          (turn) => turn.id === resolvedTurnId,
        );
        const materializedTurn = nextTurnsById.get(resolvedTurnId) ?? null;
        const promptTurn = findTurnWithUserMessage(
          nextDetailWithLiveTimestamps.turns,
          current.prompt,
        );
        const hasMaterializedPrompt = Boolean(promptTurn);
        if (promptTurn && !current.serverTurnId) {
          return {
            ...current,
            id: promptTurn.id,
            serverTurnId: promptTurn.id,
            status:
              current.status === 'failed'
                ? current.status
                : promptTurn.status === 'inProgress'
                  ? 'inProgress'
                  : current.status,
          };
        }
        if (materializedTurn && current.serverTurnId) {
          const hasMaterializedUserMessage = materializedTurn.items.some(
            (item) => item.kind === 'userMessage',
          );
          if (!hasMaterializedUserMessage) {
            return {
              ...current,
              id: materializedTurn.id,
              serverTurnId: materializedTurn.id,
              status:
                current.status === 'failed'
                  ? current.status
                  : materializedTurn.status === 'inProgress'
                    ? 'inProgress'
                    : materializedTurn.status === 'failed'
                      ? 'failed'
                      : 'completed',
            };
          }

          return materializedTurn.status === 'inProgress'
            ? {
                ...current,
                id: materializedTurn.id,
                serverTurnId: materializedTurn.id,
                status: current.status === 'failed' ? current.status : 'inProgress',
              }
            : null;
        }
        if (
          !current.serverTurnId &&
          promptHasPhotoPlaceholder(current.prompt) &&
          nextDetailWithLiveTimestamps.thread.activeTurnId &&
          nextDetailWithLiveTimestamps.thread.status === 'running'
        ) {
          const activeTurn = nextTurnsById.get(
            nextDetailWithLiveTimestamps.thread.activeTurnId,
          );
          if (activeTurn && turnHasPhotoAttachment(activeTurn)) {
            return {
              ...current,
              id: activeTurn.id,
              serverTurnId: activeTurn.id,
              status: current.status === 'failed' ? current.status : 'inProgress',
            };
          }
        }
        return hasMaterializedTurn || (threadHasEnded && hasMaterializedPrompt)
          ? null
          : current;
      });
      if (
        threadHasEnded ||
        (terminalTurnPendingRef.current &&
          nextDetailWithLiveTimestamps.turns.some(
            (turn) => turn.id === terminalTurnPendingRef.current,
          ))
      ) {
        terminalTurnPendingRef.current = null;
        clearBufferedLiveOutput();
        setLiveOutput('');
        setLivePlan(null);
        setLiveItems(null);
      }
    },
    [clearBufferedLiveOutput, routeKey, id, setDetail, setThreads],
  );
  applyDetailResponseRef.current = applyDetailResponse;

  useEffect(() => {
    detailRef.current = detail;
  }, [detail]);

  useThreadTabStatus(detail?.thread ?? null);

  const refreshCapabilities = useCallback(async () => {
    if (!id) return;
    const snapshot = await fetchThreadCapabilitySnapshot(id);
    if (activeThreadIdRef.current !== id) return;
    setBackendCapabilities(snapshot.effectiveCapabilities ?? UNAVAILABLE_AGENT_CAPABILITIES);
    setBackendManagementSchema((current) => ({
      hostConfigFiles: current?.hostConfigFiles ?? [],
      toolboxItems: snapshot.toolboxItems ?? [],
      hookCommandTemplates: current?.hookCommandTemplates ?? [],
      providerConfigFormat: current?.providerConfigFormat ?? 'none',
      mcpConfigFormat: current?.mcpConfigFormat ?? 'none',
      configArchives: current?.configArchives ?? false,
      buildRestart: current?.buildRestart ?? false,
    }));
  }, [id]);
  // The page context reads the first snapshot. Afterwards the thread's events,
  // its connection, a reconnect or a return to the tab refresh it; failures
  // keep the last snapshot.
  const capabilityRefresh = useEventDrivenRefresh({
    enabled: Boolean(id),
    refresh: refreshCapabilities,
    fallbackMs: CAPABILITY_FALLBACK_REFRESH_MS,
    coalesceMs: 500,
  });
  const capabilityRefreshRef = useRef(capabilityRefresh);
  capabilityRefreshRef.current = capabilityRefresh;
  const threadConnectionKey = detail?.thread.id === id && detail.thread.isLoaded
    ? `${id}:${detail.thread.provider}:${detail.thread.agentId ?? ''}:${detail.thread.providerSessionId ?? ''}`
    : null;
  const previousConnectionKeyRef = useRef<{ id: string; key: string | null } | null>(null);
  useEffect(() => {
    const previous = previousConnectionKeyRef.current;
    previousConnectionKeyRef.current = { id, key: threadConnectionKey };
    // Only a connection made while the page is open; the first snapshot covers
    // a thread that was already connected.
    if (!threadConnectionKey || !previous || previous.id !== id || previous.key === threadConnectionKey) return;
    const timers = CAPABILITY_SETTLE_DELAYS_MS.map(delay =>
      window.setTimeout(() => capabilityRefreshRef.current.request(0), delay));
    return () => timers.forEach(timer => window.clearTimeout(timer));
  }, [id, threadConnectionKey]);

  const loadPageContext = useCallback(
    async ({ seedThread }: { seedThread?: ThreadDto | null } = {}) => {
      const requestId = pageContextRequestIdRef.current + 1;
      pageContextRequestIdRef.current = requestId;
      const provider =
        seedThread?.provider ?? detailRef.current?.thread.provider ?? 'codex';
      const agentId = seedThread?.agentId ?? detailRef.current?.thread.agentId ?? null;
      const cwd = detailRef.current?.workspace.absPath ?? null;
      const threadId = seedThread?.id ?? detailRef.current?.thread.id;
      setModelOptions([]);
      const fallbackModels = () => fetchAgentBackendModelsFor(provider, {
        ...(provider === 'acp' && agentId ? { agentId } : {}),
        cwd,
      });
      const modelRequest = threadId ? fetchThreadModels(threadId).catch(error => {
        if (error instanceof ApiError && error.statusCode === 404) return fallbackModels();
        throw error;
      }) : fallbackModels();
      const agentRequest = provider === 'acp'
        ? fetchAgentBackendAgents(provider)
        : Promise.resolve([] as ModelOptionDto[]);
      const capabilityRequest = threadId
        ? fetchThreadCapabilitySnapshot(threadId)
        : Promise.resolve(null);
      // Claim the provider before awaiting: detail loads that finish meanwhile
      // (socket open, recovery) must not start the same bundle again.
      pageContextProviderRef.current = provider;

      const [threadResult, statusResult, modelResult, agentResult, capabilityResult] = await Promise.allSettled([
        fetchThreads(true).catch(() => threadId ? fetchThreadGroup(seedThread?.rootThreadId ?? threadId).catch(() => fetchThreadGroup(threadId)) : []),
        fetchAgentBackendStatus(provider),
        modelRequest,
        agentRequest,
        capabilityRequest,
      ]);

      if (pageContextRequestIdRef.current !== requestId) {
        return;
      }
      if (statusResult.status === 'rejected' && modelResult.status === 'rejected') {
        // Let the next detail load retry the context.
        pageContextProviderRef.current = null;
      }

      if (threadResult.status === 'fulfilled') {
        threadListRef.current.markFresh();
        setThreads(
          seedThread
            ? mergeThreadIntoList(threadResult.value, seedThread)
            : threadResult.value,
        );
      } else if (seedThread) {
        setThreads((current) => mergeThreadIntoList(current, seedThread));
      }

      if (statusResult.status === 'fulfilled') {
        setStatus(statusResult.value.status);
        setBackendCapabilities(statusResult.value.capabilities);
        setBackendManagementSchema(statusResult.value.managementSchema);
      }

      if (modelResult.status === 'fulfilled') {
        setModelOptions(modelResult.value);
      }
      if (agentResult.status === 'fulfilled') {
        setAgentOptions(agentResult.value);
      }
      if (capabilityResult.status === 'fulfilled' && capabilityResult.value) {
        capabilityRefreshRef.current.markFresh();
        const capabilitySnapshot = capabilityResult.value;
        setBackendCapabilities(
          capabilitySnapshot.effectiveCapabilities ??
            UNAVAILABLE_AGENT_CAPABILITIES,
        );
        setBackendManagementSchema((current) => ({
          hostConfigFiles: current?.hostConfigFiles ?? [],
          toolboxItems:
            capabilitySnapshot.toolboxItems ?? current?.toolboxItems ?? [],
          hookCommandTemplates: current?.hookCommandTemplates ?? [],
          providerConfigFormat: current?.providerConfigFormat ?? 'none',
          mcpConfigFormat: current?.mcpConfigFormat ?? 'none',
          configArchives: current?.configArchives ?? false,
          buildRestart: current?.buildRestart ?? false,
        }));
        if (!capabilitySnapshot.effectiveCapabilities) {
          setError((current) => current ??
            `The selected ACP agent is ${capabilitySnapshot.availability.replaceAll('_', ' ')}. Install or repair its adapter before continuing.`);
        }
      }
    },
    [relayRouteDeviceId, setThreads],
  );

  const loadThreadDetail = useCallback(
    async ({
      showLoading = true,
      clearError = true,
      reportError = true,
      limit = INITIAL_DETAIL_TURN_PAGE_SIZE,
    }: {
      showLoading?: boolean;
      clearError?: boolean;
      reportError?: boolean;
      limit?: number;
    } = {}) => {
      if (detailMutationInFlightRef.current > 0) {
        detailRefreshPendingRef.current = true;
        return;
      }

      const requestId = loadRequestIdRef.current + 1;
      loadRequestIdRef.current = requestId;
      if (showLoading) {
        setLoading(true);
      }
      if (clearError) {
        setError(null);
      }

      detailLoadsInFlightRef.current += 1;
      detailLoadStartedAtRef.current = Date.now();
      try {
        const prefetched = limit === INITIAL_DETAIL_TURN_PAGE_SIZE ? pendingThreadDetails.get(routeKey) : undefined;
        const detailResponse = await (prefetched?.catch(() => fetchThreadDetail(id, { limit })) ?? fetchThreadDetail(id, {
          limit,
        }));
        if (loadRequestIdRef.current !== requestId || activeRouteRef.current !== routeKey) {
          return;
        }

        applyDetailResponse(detailResponse);
        if (pageContextProviderRef.current !== detailResponse.thread.provider) {
          void loadPageContext({ seedThread: detailResponse.thread });
        }
        if (recheckAfterFirstOpenRef.current) {
          recheckAfterFirstOpenRef.current = false;
          if (threadIsActive(detailResponse)) {
            window.setTimeout(() => {
              void loadThreadDetail({ showLoading: false, clearError: false, reportError: false });
            }, 0);
          }
        }
      } catch (caught) {
        if (loadRequestIdRef.current !== requestId || activeRouteRef.current !== routeKey) return;
        if (hostScope && isInvisibleThreadError(caught)) {
          await workbenchNavigation.onThreadRemoved(`local:${id}`);
          setThreads(previous => previous.filter(thread => thread.id !== id));
          navigate('/threads', { replace: true });
          return;
        }
        if (!reportError) return;
        setError(
          caught instanceof Error
            ? caught.message
            : translate("workbench.unableToLoadThreadDetail"),
        );
      } finally {
        detailLoadsInFlightRef.current -= 1;
        if (loadRequestIdRef.current === requestId) {
          setLoading(false);
        }
      }
    },
    [applyDetailResponse, id, loadPageContext, routeKey, hostScope, navigate],
  );

  // A first load that failed retries with backoff while the tab is visible.
  const initialLoadFailed = !loading && detail === null && error !== null;
  useEffect(() => {
    if (!initialLoadFailed) return;
    const delay = Math.min(60_000, 3_000 * 2 ** initialLoadRetriesRef.current);
    let timer: number | null = null;
    const retry = () => {
      if (document.visibilityState === 'hidden') return;
      document.removeEventListener('visibilitychange', retry);
      initialLoadRetriesRef.current += 1;
      void loadThreadDetail({ showLoading: false });
    };
    timer = window.setTimeout(() => {
      timer = null;
      if (document.visibilityState === 'hidden') document.addEventListener('visibilitychange', retry);
      else retry();
    }, delay);
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', retry);
    };
  }, [initialLoadFailed, loadThreadDetail]);

  useEffect(() => {
    const refresh = () => {void loadThreadDetail({showLoading:false,clearError:false});};
    window.addEventListener('model-pricing-updated', refresh);
    // Keeping completed turns open asks the summary for their narration.
    window.addEventListener(TRANSCRIPT_PREFERENCE_EVENT, refresh);
    return () => {
      window.removeEventListener('model-pricing-updated', refresh);
      window.removeEventListener(TRANSCRIPT_PREFERENCE_EVENT, refresh);
    };
  }, [loadThreadDetail]);

  const runDetailMutation = useCallback(
    <T extends ThreadDetailDto,>(operation: () => Promise<T>) => {
      const scheduled = detailMutationChainRef.current.then(async () => {
        detailMutationInFlightRef.current += 1;
        // Invalidate a detail request that began before this mutation. Its
        // response describes pre-mutation state and must never overwrite the
        // mutation result.
        loadRequestIdRef.current += 1;
        try {
          const updated = await operation();
          if (updated.thread.id === activeThreadIdRef.current) {
            applyDetailResponse(updated);
          }
          return updated;
        } finally {
          detailMutationInFlightRef.current -= 1;
          window.setTimeout(() => {
            if (
              detailMutationInFlightRef.current === 0 &&
              detailRefreshPendingRef.current
            ) {
              detailRefreshPendingRef.current = false;
              void loadThreadDetail({
                showLoading: false,
                clearError: false,
                reportError: false,
              });
            }
          }, 0);
        }
      });

      detailMutationChainRef.current = scheduled.then(
        () => undefined,
        () => undefined,
      );
      return scheduled;
    },
    [applyDetailResponse, loadThreadDetail],
  );

  useEffect(() => {
    const current = detailRef.current;
    if (!current || current.thread.id !== id || !relayThreadCanControl) return;
    // One attempt per visit, including StrictMode effect replay. A failed
    // connection remains retryable through the connection indicator.
    if (autoConnectionAttempt.current?.key !== routeKey) {
      const needsConnection = !current.thread.isLoaded || current.thread.status === 'recovering';
      autoConnectionAttempt.current = {
        key: routeKey,
        promise: needsConnection ? runDetailMutation(() => {
          if (activeRouteRef.current !== routeKey) return Promise.resolve(current);
          return resumeThread(id, current.thread.model ? { model: current.thread.model } : {});
        }) : null,
      };
    }
    const pending = autoConnectionAttempt.current.promise;
    if (!pending) return;
    let cancelled = false;
    setAutoConnectingRoute(routeKey);
    void pending.catch((caught) => {
      if (!cancelled) setError(caught instanceof Error ? caught.message : translate("workbench.unableToConnectToThisThread"));
    }).finally(() => {
      if (!cancelled) setAutoConnectingRoute(null);
    });
    return () => { cancelled = true; };
  }, [routeKey, id, detail?.thread.id, relayThreadCanControl, runDetailMutation]);

  const syncRealtimeConnectionState = useCallback(() => {
    const socketState = supervisorSocketRef.current?.readyState ?? SOCKET_CLOSED;
    const socketOpen = socketState === SOCKET_OPEN;
    const browserOnline = supervisorBrowserOnlineRef.current;
    const now = Date.now();
    const hasRecentHealth =
      supervisorHealthOkAtRef.current !== null &&
      now - Date.parse(supervisorHealthOkAtRef.current) <= SUPERVISOR_CONNECTION_STALE_MS;
    const hasRecentPong =
      supervisorHeartbeatRef.current.responsive(now);

    let status: RealtimeConnectionStatus;
    if (!browserOnline) {
      status = 'offline';
    } else if (socketOpen && hasRecentPong) {
      status = 'connected';
    } else if (
      socketState === SOCKET_CONNECTING ||
      supervisorReconnectTimerRef.current !== null ||
      hasRecentHealth ||
      hasRecentPong ||
      supervisorHealthInFlightRef.current
    ) {
      status = 'reconnecting';
    } else {
      status = 'checking';
    }

    setRealtimeConnection((current) => {
      if (
        current.status === status &&
        current.browserOnline === browserOnline &&
        current.healthOk === hasRecentHealth &&
        current.socketOpen === socketOpen &&
        current.lastHealthyAt === supervisorHealthOkAtRef.current
      ) {
        return current;
      }

      return {
        status,
        browserOnline,
        healthOk: hasRecentHealth,
        socketOpen,
        lastHealthyAt: supervisorHealthOkAtRef.current,
      };
    });
  }, []);

  useEffect(() => {
    loadRequestIdRef.current += 1;
    pageContextRequestIdRef.current += 1;
    pageContextProviderRef.current = null;
    recheckAfterFirstOpenRef.current = false;
    initialLoadRetriesRef.current = 0;
    const recent = recentThreadDetails.get(`${relayRouteDeviceId ?? 'local'}:${id}`) ?? null;
    setDetail(null);
    setError(null);
    setLoading(!recent);
    setLoadingEarlier(false);
    setMetaSessionCopyState('idle');
    setOptimisticTurn(null);
    setOptimisticSteers([]);
    setLiveItems(null);
    pendingThreadSettingsRef.current = null;
    terminalTurnPendingRef.current = null;
    resolvedRequestIdsRef.current = new Set();
    supervisorHealthOkAtRef.current = null;
    supervisorHeartbeatRef.current.reset();
    supervisorRecoveryPendingRef.current = false;
    supervisorBrowserOnlineRef.current =
      typeof navigator === 'undefined' ? true : navigator.onLine;
    setRealtimeConnection({
      status: supervisorBrowserOnlineRef.current ? 'checking' : 'offline',
      browserOnline: supervisorBrowserOnlineRef.current,
      healthOk: false,
      socketOpen: false,
      lastHealthyAt: null,
    });
    if (recent) applyDetailResponseRef.current(recent);
  }, [id, relayRouteDeviceId]);

  useEffect(() => {
    if (metaSessionCopyState === 'idle') {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      setMetaSessionCopyState('idle');
    }, metaSessionCopyState === 'copied' ? 1200 : 1600);

    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [metaSessionCopyState]);

  useEffect(() => {
    if (typeof document === 'undefined') {
      return;
    }

    const { documentElement, body } = document;
    documentElement.classList.add('thread-detail-scroll-locked');
    body.classList.add('thread-detail-scroll-locked');

    return () => {
      documentElement.classList.remove('thread-detail-scroll-locked');
      body.classList.remove('thread-detail-scroll-locked');
    };
  }, []);

  useEffect(() => {
    void loadThreadDetail({
      showLoading: !recentThreadDetails.has(routeKey),
      limit: INITIAL_DETAIL_TURN_PAGE_SIZE,
    });
  }, [loadThreadDetail]);

  useEffect(() => {
    let isDisposed = false;
    let heartbeatIntervalId: number | null = null;
    let detailRefreshTimer: number | null = null;
    const turnEndTimers = new Set<number>();
    let socketOpenedBefore = false;

    // Events often arrive together (turn start and status); read once.
    const refreshThreadDetailSilently = () => {
      if (interruptingRef.current || detailRefreshTimer !== null) {
        return;
      }
      detailRefreshTimer = window.setTimeout(() => {
        detailRefreshTimer = null;
        if (isDisposed || interruptingRef.current) return;
        void loadThreadDetail({
          showLoading: false,
          clearError: false,
          reportError: false,
        });
      }, DETAIL_REFRESH_COALESCE_MS);
    };

    const clearReconnectTimer = () => {
      if (supervisorReconnectTimerRef.current !== null) {
        window.clearTimeout(supervisorReconnectTimerRef.current);
        supervisorReconnectTimerRef.current = null;
      }
    };

    const scheduleReconnect = () => {
      if (
        isDisposed ||
        !supervisorBrowserOnlineRef.current ||
        supervisorReconnectTimerRef.current !== null
      ) {
        return;
      }

      supervisorReconnectTimerRef.current = window.setTimeout(() => {
        supervisorReconnectTimerRef.current = null;
        if (isDisposed) {
          return;
        }

        connectSocket();
      }, SUPERVISOR_SOCKET_RECONNECT_DELAY_MS);
      syncRealtimeConnectionState();
    };

    const closeSupervisorSocket = () => {
      const activeSocket = supervisorSocketRef.current;
      supervisorSocketRef.current = null;
      if (activeSocket) {
        try {
          activeSocket.close();
        } catch {
          // Ignore socket close errors during reconnect cleanup.
        }
      }
    };

    const handleSocketEvent = (event: ThreadEventEnvelope) => {
      referenceEventRef.current(event);
      announceHarnessRuntimeEvent(event);
      if (event.threadId !== id) {
        // The current thread reaches the list through its own detail reads.
        if (THREAD_LIST_EVENT_TYPES.has(event.type) && !isPendingSteerUpdate(event)) {
          threadListRef.current.refresh();
        }
        return;
      }
      if (CAPABILITY_EVENT_TYPES.has(event.type) && !isPendingSteerUpdate(event)) {
        capabilityRefreshRef.current.request();
      }
      // Record receipt, not rendering, polling or socket keepalive time.
      if (event.type.startsWith('thread.item.') || event.type.startsWith('thread.turn.')
        || ['thread.output.delta', 'thread.plan.updated', 'thread.context.updated', 'thread.subagents.updated', 'thread.request.created'].includes(event.type)) {
        const turnId = 'turnId' in event.payload && typeof event.payload.turnId === 'string' ? event.payload.turnId : detailRef.current?.thread.activeTurnId;
        if (turnId) setBackendProgress({ threadId: id!, turnId, receivedAt: new Date().toISOString() });
      }

      if (
        event.type === 'thread.output.delta' &&
        typeof event.payload.delta === 'string'
      ) {
        const eventTurnId =
          typeof event.payload.turnId === 'string' ? event.payload.turnId : null;
        const itemId =
          typeof event.payload.itemId === 'string' ? event.payload.itemId : null;
        const sequence =
          typeof event.payload.sequence === 'number' &&
          Number.isFinite(event.payload.sequence)
            ? event.payload.sequence
            : null;
        if (eventTurnId && itemId) {
          appendLiveAgentDelta(
            eventTurnId,
            itemId,
            event.payload.delta,
            sequence,
            event.payload.createdAt ?? event.timestamp,
            event.payload.text,
          );
        } else {
          queueLiveOutputDelta(event.payload.delta);
        }
        if (eventTurnId) {
          setOptimisticTurn((current) =>
            current &&
            (current.serverTurnId === null || current.serverTurnId === eventTurnId)
              ? {
                  ...current,
                  serverTurnId: eventTurnId,
                  id: eventTurnId,
                  status: current.status === 'failed' ? current.status : 'inProgress',
                  tokenUsage: current.tokenUsage,
                }
            : current,
          );
        }
      }

      if (event.type === 'thread.context.updated') {
        const nextContextUsage =
          event.payload.contextUsage &&
          typeof event.payload.contextUsage === 'object'
            ? event.payload.contextUsage
            : null;
        if (nextContextUsage) {
          const normalizedContextUsage =
            nextContextUsage as NonNullable<ThreadDto['contextUsage']>;
          setDetail((current) =>
            current
              ? {
                  ...current,
                  thread: {
                    ...current.thread,
                    contextUsage: normalizedContextUsage,
                  },
                }
              : current,
          );
          setThreads((current) =>
            current.map((entry) =>
              entry.id === id
                ? {
                    ...entry,
                    contextUsage: normalizedContextUsage,
                  }
                : entry,
            ),
          );
        }
      }

      if (event.type === 'thread.subagents.updated') {
        const activeSubagents = Array.isArray(event.payload.activeSubagents)
          ? event.payload.activeSubagents
          : [];
        setDetail((current) =>
          current ? { ...current, activeSubagents } : current,
        );
        if (detailRef.current) {
          detailRef.current = { ...detailRef.current, activeSubagents };
        }
      }

      if (
        event.type === 'thread.turn.token.updated' &&
        typeof event.payload.turnId === 'string' &&
        event.payload.tokenUsage &&
        typeof event.payload.tokenUsage === 'object'
      ) {
        const eventTurnId = event.payload.turnId;
        const tokenUsage = event.payload.tokenUsage as ThreadTurnTokenUsageDto;
        const metadata = {
          ...(typeof event.payload.model === 'string'
            ? { model: event.payload.model }
            : {}),
          ...(typeof event.payload.reasoningEffort === 'string'
            ? { reasoningEffort: event.payload.reasoningEffort as ThreadDto['reasoningEffort'] }
            : {}),
          ...(typeof event.payload.reasoningEffortAvailable === 'boolean'
            ? { reasoningEffortAvailable: event.payload.reasoningEffortAvailable }
            : {}),
        };
        const priceEstimate =
          event.payload.priceEstimate &&
          typeof event.payload.priceEstimate === 'object'
            ? (event.payload.priceEstimate as ThreadTurnPriceEstimateDto)
            : null;

        setDetail((current) => {
          if (!current) {
            return current;
          }

          const nextTurns = mergeTurnTokenUsage(
            current.turns,
            eventTurnId,
            tokenUsage,
            priceEstimate,
            metadata,
          );

          return nextTurns === current.turns
            ? current
            : {
                ...current,
                turns: nextTurns,
              };
        });

        setOptimisticTurn((current) =>
          current &&
          (current.serverTurnId === eventTurnId || current.id === eventTurnId)
            ? {
                ...current,
                ...metadata,
                tokenUsage,
                priceEstimate,
              }
            : current,
        );
      }

      if (event.type === 'thread.persistence.failed') {
        setError(String(event.payload.message ?? translate("workbench.unableToSaveOutput")));
      }
      if (event.type === 'thread.updated' && Array.isArray(event.payload.pendingSteers)) {
        const pendingSteers = event.payload.pendingSteers as ThreadDetailDto['pendingSteers'];
        setDetail(current => current ? {...current, pendingSteers} : current);
        if (detailRef.current) detailRef.current = {...detailRef.current, pendingSteers};
        const acknowledged = new Set(pendingSteers.map(item => item.clientRequestId));
        setOptimisticSteers(current => current.filter(item => !acknowledged.has(item.clientRequestId)));
        return;
      }
      if (
        event.type === 'thread.turn.started' ||
        event.type === 'thread.turn.completed' ||
        event.type === 'thread.turn.failed' ||
        event.type === 'thread.updated' ||
        event.type === 'thread.goal.updated' ||
        event.type === 'thread.goal.cleared' ||
        event.type === 'thread.request.created' ||
        event.type === 'thread.request.resolved'
      ) {
        if (event.type === 'thread.updated' && ['children_changed','child_deleted'].includes(String(event.payload.reason))) {
          void loadPageContext({seedThread:detailRef.current?.thread ?? null});
        }
        if (event.type === 'thread.goal.updated') {
          const goal =
            event.payload.goal && typeof event.payload.goal === 'object'
              ? (event.payload.goal as NonNullable<ThreadDetailDto['goal']>)
              : null;
          const goalHistory =
            Array.isArray(event.payload.goalHistory)
              ? (event.payload.goalHistory as NonNullable<ThreadDetailDto['goalHistory']>)
              : null;
          setGoalState({
            status: 'ready',
            data: goal,
            error: null,
          });
          setDetail((current) =>
            current
              ? goalHistory
                ? {
                    ...current,
                    goal,
                    goalHistory,
                  }
                : goal
                  ? {
                      ...current,
                      goal,
                      goalHistory: mergeGoalHistory(current.goalHistory ?? [], goal),
                    }
                  : {
                      ...current,
                      goal,
                    }
              : current,
          );
        }
        if (event.type === 'thread.goal.cleared') {
          const goalHistory =
            Array.isArray(event.payload.goalHistory)
              ? (event.payload.goalHistory as NonNullable<ThreadDetailDto['goalHistory']>)
              : null;
          setGoalState({
            status: 'ready',
            data: null,
            error: null,
          });
          setDetail((current) =>
            current
              ? goalHistory
                ? {
                    ...current,
                    goal: null,
                    goalHistory,
                  }
                : {
                    ...current,
                    goal: null,
                  }
              : current,
          );
        }
        if (event.type !== 'thread.turn.completed') {
          refreshThreadDetailSilently();
        }
        if (event.type === 'thread.turn.started') {
          clearBufferedLiveOutput();
          setLiveOutput('');
          setLiveItems(null);
          terminalTurnPendingRef.current = null;
          const eventTurnId =
            typeof event.payload.turnId === 'string' ? event.payload.turnId : null;
          if (eventTurnId) {
            setOptimisticTurn((current) =>
              current
                ? {
                    ...current,
                    serverTurnId: eventTurnId,
                    id: eventTurnId,
                    status: current.status === 'failed' ? current.status : 'inProgress',
                    error: null,
                    tokenUsage: current.tokenUsage,
                  }
                : current,
            );
          }
        }
        if (
          event.type === 'thread.turn.completed' ||
          event.type === 'thread.turn.failed'
        ) {
          // The Supervisor follows a turn end with thread.updated once it has
          // persisted the turn. Read the summary anyway if none follows.
          const endedAt = Date.now();
          const timer = window.setTimeout(() => {
            turnEndTimers.delete(timer);
            if (!isDisposed && detailLoadStartedAtRef.current < endedAt) refreshThreadDetailSilently();
          }, TURN_END_REFRESH_GRACE_MS);
          turnEndTimers.add(timer);
          clearBufferedLiveOutput();
          setLiveOutput('');
          const eventTurnId =
            typeof event.payload.turnId === 'string' ? event.payload.turnId : null;
          if (eventTurnId) {
            terminalTurnPendingRef.current = eventTurnId;
            const terminalStatus =
              event.type === 'thread.turn.failed'
                ? 'failed'
                : event.payload.status;
            const terminalError =
              typeof event.payload.error === 'string'
                ? event.payload.error
                : event.type === 'thread.turn.failed'
                  ? translate("workbench.unableToCompleteTheTurn")
                  : null;
            setDetail((current) =>
              current
                ? {
                    ...current,
                    turns: current.turns.map((turn) =>
                      turn.id === eventTurnId
                        ? {
                            ...turn,
                            status: terminalStatus,
                            completedAt: terminalStatus === 'recovering' ? null : event.timestamp,
                            error: terminalError,
                          }
                        : turn,
                    ),
                  }
                : current,
            );
            setOptimisticTurn((current) =>
              current &&
              (current.serverTurnId === eventTurnId || current.id === eventTurnId)
                ? {
                    ...current,
                    status: terminalStatus,
                    error: terminalError,
                    tokenUsage: current.tokenUsage,
                  }
                : current,
            );
          }
        }
      }

      if (
        event.type === 'thread.request.created' &&
        isThreadActionRequest(event.payload.request)
      ) {
        resolvedRequestIdsRef.current.delete(event.payload.request.id);
        setDetail((current) =>
          current ? mergePendingRequestIntoDetail(current, event.payload.request) : current,
        );
      }

      if (
        event.type === 'thread.request.resolved' &&
        typeof event.payload.requestId === 'string'
      ) {
        const requestId = event.payload.requestId;
        resolvedRequestIdsRef.current.add(requestId);
        setDetail((current) =>
          current ? removePendingRequestFromDetail(current, requestId) : current,
        );
      }

      if (
        (event.type === 'thread.item.started' ||
          event.type === 'thread.item.completed') &&
        event.payload.item &&
        typeof event.payload.item === 'object' &&
        typeof event.payload.turnId === 'string'
      ) {
        const eventTurnId = event.payload.turnId;
        const liveItem = event.payload.item as ThreadDetailDto['turns'][number]['items'][number];
        if (typeof liveItem.id === 'string' && typeof liveItem.text === 'string') {
          upsertLiveTimelineItem(eventTurnId, liveItem);
        }
      }

      if (
        event.type === 'thread.plan.updated' &&
        Array.isArray(event.payload.plan)
      ) {
        setLivePlan({
          turnId: String(event.payload.turnId ?? ''),
          explanation:
            typeof event.payload.explanation === 'string'
              ? event.payload.explanation
              : null,
          plan: event.payload.plan as Array<{ step: string; status: string }>,
        });
      }
    };

    const sendSupervisorPing = () => {
      const activeSocket = supervisorSocketRef.current;
      if (!activeSocket || activeSocket.readyState !== SOCKET_OPEN) {
        return;
      }

      try {
        activeSocket.send(
          JSON.stringify({
            type: 'supervisor.ping',
            timestamp: new Date().toISOString(),
          }),
        );
      } catch {
        supervisorRecoveryPendingRef.current = true;
        closeSupervisorSocket();
        scheduleReconnect();
        syncRealtimeConnectionState();
      }
    };

    const connectSocket = () => {
      if (isDisposed || !supervisorBrowserOnlineRef.current) {
        syncRealtimeConnectionState();
        return;
      }
      if (document.visibilityState === 'hidden') return;

      const socketState = supervisorSocketRef.current?.readyState ?? SOCKET_CLOSED;
      if (socketState === SOCKET_CONNECTING || socketState === SOCKET_OPEN) {
        syncRealtimeConnectionState();
        return;
      }

      const nextSocket = connectSupervisorEvents(handleSocketEvent);
      supervisorSocketRef.current = nextSocket;
      syncRealtimeConnectionState();

      nextSocket.addEventListener('message', (message) => {
        if (supervisorSocketRef.current !== nextSocket) {
          return;
        }

        try {
          const parsed = JSON.parse(
            message.data as string,
          ) as SupervisorSocketServerEnvelope;
          if (
            parsed.type === 'supervisor.connected' ||
            parsed.type === 'supervisor.pong'
          ) {
            supervisorHeartbeatRef.current.receive(Date.now());
            syncRealtimeConnectionState();
          }
        } catch {
          // Ignore malformed socket payloads.
        }
      });

      nextSocket.addEventListener('open', () => {
        if (supervisorSocketRef.current !== nextSocket) {
          return;
        }

        supervisorHeartbeatRef.current.probe(Date.now());
        const reconnected = socketOpenedBefore;
        socketOpenedBefore = true;
        supervisorRecoveryPendingRef.current = false;
        const known = detailRef.current?.thread.id === id ? detailRef.current : null;
        if (reconnected) {
          // Events may have been missed while the socket was down.
          refreshThreadDetailSilently();
          threadListRef.current.refresh(0);
          capabilityRefreshRef.current.request(0);
        } else if (known) {
          // The first summary is current unless a running turn's events
          // slipped in before the socket subscribed.
          if (threadIsActive(known)) refreshThreadDetailSilently();
        } else if (detailLoadsInFlightRef.current > 0) {
          recheckAfterFirstOpenRef.current = true;
        } else {
          refreshThreadDetailSilently();
        }
        threadView.socketOpened();
        sendSupervisorPing();
        syncRealtimeConnectionState();
      });

      nextSocket.addEventListener('close', () => {
        if (isDisposed || supervisorSocketRef.current !== nextSocket) return;
        closeSupervisorSocket();
        supervisorRecoveryPendingRef.current = true;
        scheduleReconnect();
        syncRealtimeConnectionState();
      });

      nextSocket.addEventListener('error', () => {
        if (isDisposed || supervisorSocketRef.current !== nextSocket) return;
        closeSupervisorSocket();
        supervisorRecoveryPendingRef.current = true;
        scheduleReconnect();
        syncRealtimeConnectionState();
      });
    };

    const runHealthCheck = async () => {
      if (
        isDisposed ||
        !supervisorBrowserOnlineRef.current ||
        supervisorHealthInFlightRef.current
      ) {
        return;
      }

      supervisorHealthInFlightRef.current = true;
      syncRealtimeConnectionState();

      try {
        await fetchSupervisorHealth();
        supervisorHealthOkAtRef.current = new Date().toISOString();
        const socketState = supervisorSocketRef.current?.readyState ?? SOCKET_CLOSED;
        // A socket that opens refreshes the thread itself; read it here only
        // while the socket stays down.
        if (supervisorRecoveryPendingRef.current && socketState === SOCKET_CLOSED) {
          supervisorRecoveryPendingRef.current = false;
          refreshThreadDetailSilently();
        }
        if (socketState !== SOCKET_OPEN) {
          connectSocket();
        }
      } catch {
        supervisorHealthOkAtRef.current = null;
        supervisorRecoveryPendingRef.current = true;
        scheduleReconnect();
      } finally {
        supervisorHealthInFlightRef.current = false;
        syncRealtimeConnectionState();
      }
    };

    const handleBrowserOnline = () => {
      supervisorBrowserOnlineRef.current = true;
      supervisorRecoveryPendingRef.current = true;
      syncRealtimeConnectionState();
      connectSocket();
      void runHealthCheck();
    };

    const handleBrowserOffline = () => {
      supervisorBrowserOnlineRef.current = false;
      supervisorHealthOkAtRef.current = null;
      supervisorHeartbeatRef.current.reset();
      supervisorRecoveryPendingRef.current = true;
      clearReconnectTimer();
      closeSupervisorSocket();
      syncRealtimeConnectionState();
    };

    const runHeartbeat = () => {
      if (isDisposed || !supervisorBrowserOnlineRef.current) {
        syncRealtimeConnectionState();
        return;
      }
      if (document.visibilityState === 'hidden') return;

      const socketState = supervisorSocketRef.current?.readyState ?? SOCKET_CLOSED;
      if (
        socketState === SOCKET_OPEN &&
        supervisorHeartbeatRef.current.timedOut(Date.now())
      ) {
        supervisorRecoveryPendingRef.current = true;
        closeSupervisorSocket();
        scheduleReconnect();
      } else if (socketState === SOCKET_OPEN) {
        sendSupervisorPing();
      } else if (socketState !== SOCKET_CONNECTING) {
        connectSocket();
      }

      // The socket's pongs prove the Supervisor is reachable; check HTTP
      // health only while it is down, and not from a hidden tab.
      if ((supervisorSocketRef.current?.readyState ?? SOCKET_CLOSED) !== SOCKET_OPEN) {
        void runHealthCheck();
      }
      syncRealtimeConnectionState();
    };

    const handleVisibilityChange = () => {
      if (document.visibilityState !== 'hidden') {
        supervisorHeartbeatRef.current.probe(Date.now());
        runHeartbeat();
      }
    };

    window.addEventListener('online', handleBrowserOnline);
    window.addEventListener('offline', handleBrowserOffline);
    document.addEventListener('visibilitychange', handleVisibilityChange);
    connectSocket();
    heartbeatIntervalId = window.setInterval(
      runHeartbeat,
      SUPERVISOR_HEALTHCHECK_INTERVAL_MS,
    );

    return () => {
      isDisposed = true;
      window.removeEventListener('online', handleBrowserOnline);
      window.removeEventListener('offline', handleBrowserOffline);
      document.removeEventListener('visibilitychange', handleVisibilityChange);
      clearReconnectTimer();
      if (heartbeatIntervalId !== null) {
        window.clearInterval(heartbeatIntervalId);
      }
      if (detailRefreshTimer !== null) window.clearTimeout(detailRefreshTimer);
      turnEndTimers.forEach(timer => window.clearTimeout(timer));
      clearBufferedLiveOutput();
      closeSupervisorSocket();
    };
  }, [
    appendLiveAgentDelta,
    clearBufferedLiveOutput,
    id,
    loadThreadDetail,
    queueLiveOutputDelta,
    loadPageContext,
    syncRealtimeConnectionState,
    threadView,
    upsertLiveTimelineItem,
  ]);

  // Socket events drive a running turn. Poll its summary only while no socket
  // is open, after the first load, and never from a hidden tab.
  const realtimeSocketOpen = realtimeConnection.socketOpen;
  const turnActivityPending =
    detail != null &&
    (detail.thread.activeTurnId != null ||
      detail.thread.status === 'running' ||
      optimisticTurn !== null ||
      optimisticSteers.length > 0 ||
      liveOutput.length > 0 ||
      livePlan !== null ||
      liveItems !== null);
  useEffect(() => {
    if (!turnActivityPending || realtimeSocketOpen) {
      return;
    }

    const intervalId = window.setInterval(() => {
      if (document.visibilityState === 'hidden') return;
      void loadThreadDetail({
        showLoading: false,
        clearError: false,
        reportError: false,
      });
    }, ACTIVE_THREAD_REFRESH_INTERVAL_MS);

    return () => {
      window.clearInterval(intervalId);
    };
  }, [turnActivityPending, realtimeSocketOpen, loadThreadDetail]);

  const handleLoadEarlierTurns = useCallback(async () => {
    if (!detail || detail.turns.length === 0 || loadingEarlier) {
      return;
    }

    setLoadingEarlier(true);
    setError(null);

    try {
      const earliestLoadedTurnId = detail.turns[0]?.id;
      const earlier = await fetchThreadDetail(id, {
        limit: DETAIL_TURN_PAGE_SIZE,
        ...(earliestLoadedTurnId ? { beforeTurnId: earliestLoadedTurnId } : {}),
      });
      setDetail((current) =>
        current
          ? {
              ...current,
              turns: prependTurns(current.turns, earlier.turns),
              totalTurnCount: Math.max(
                current.totalTurnCount ?? current.turns.length,
                earlier.totalTurnCount ?? earlier.turns.length,
              ),
            }
          : earlier,
      );
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : translate("workbench.unableToLoadEarlierTurns"),
      );
    } finally {
      setLoadingEarlier(false);
    }
  }, [detail, id, loadingEarlier]);

  async function performPromptSubmission(input: SendThreadPromptRequestInput) {
    if (activeView === 'shell') {
      if (detail?.thread.isLoaded === false) {
        await handleThreadConnectionToggle({ attachShell: true });
        return false;
      }

      let attemptedShellConnection = false;
      if (shellControlState?.shellInputEnabled !== true) {
        if (
          shellControlState?.loading === false &&
          shellControlState?.isConnecting !== true &&
          shellControlState?.status !== 'creating' &&
          shellControlState?.status !== 'workspace_missing'
        ) {
          await shellPanelRef.current?.toggleConnection();
          attemptedShellConnection = true;
        }
        if (shellControlState?.isConnecting === true) {
          setError(translate("workbench.connectingToTheShellTryAgainAfter"));
          return false;
        }
      }

      const sent = shellPanelRef.current?.sendCommand(input.prompt) ?? false;
      if (!sent) {
        setError(
          attemptedShellConnection
            ? translate("workbench.shellIsStillAttachingTryAgainAfter")
            : translate("workbench.connectTheShellBeforeSendingCommands"),
        );
        return false;
      } else {
        setError(null);
      }
      return true;
    }

    if (promptDisabledReason) {
      setError(promptDisabledReason);
      return false;
    }
    if ((input.attachments?.length ?? 0) > 10) {
      setError(translate("workbench.aPromptCanIncludeAtMost10"));
      return false;
    }
    if (input.delivery === 'steer' && detailRef.current?.thread.status === 'running'
      && backendCapabilities?.turns.steer === false) {
      setError(translate("workbench.thisBackendDoesNotSupportSteeringAn"));
      return false;
    }

    setBusy(true);
    setError(null);
    setScrollRequestKey((current) => current + 1);
    const activeDetail = detailRef.current;
    const effectiveThread = activeDetail
      ? {
          ...activeDetail.thread,
          ...(pendingThreadSettingsRef.current ?? {}),
        }
      : null;
    const optimisticModel = effectiveThread?.model ?? null;
    const optimisticReasoningEffort = effectiveThread?.reasoningEffort ?? null;
    const clientRequestId = createClientRequestId();
    const optimisticTurnId = `optimistic-${Date.now()}`;
    const optimisticSteerId = `optimistic-steer-${clientRequestId}`;
    const optimisticStartedAt = new Date().toISOString();
    let optimisticAttachmentPreviews: OptimisticAttachmentPreview[] = [];

    try {
      let currentDetail = detailRef.current;
      if (currentDetail && !currentDetail.thread.isLoaded && currentDetail.thread.status !== 'recovering') {
        const resumeSeedThread = {
          ...currentDetail.thread,
          ...(pendingThreadSettingsRef.current ?? {}),
        };
        const resumed = await resumeThread(
          id,
          {
            ...(currentDetail.thread.model ? { model: currentDetail.thread.model } : {}),
          },
        );
        if (activeRouteRef.current !== routeKey) return false;
        const resumedDetail = {
          ...resumed,
          thread: {
            ...resumed.thread,
            model: resumeSeedThread.model ?? resumed.thread.model,
            reasoningEffort:
              resumeSeedThread.reasoningEffort ?? resumed.thread.reasoningEffort,
            collaborationMode:
              resumeSeedThread.collaborationMode ??
              resumed.thread.collaborationMode,
          },
        };
        currentDetail = resumedDetail;
        detailRef.current = resumedDetail;
        setDetail((current) =>
          current
            ? {
                ...resumedDetail,
                turns: appendLatestTurns(
                  current.turns,
                  resumedDetail.turns,
                  resumedDetail.thread.activeTurnId,
                ),
              }
            : resumedDetail,
        );
        setThreads((current) =>
          current.map((entry) =>
            entry.id === resumedDetail.thread.id ? resumedDetail.thread : entry,
          ),
        );
      }

      const currentEffectiveThread = currentDetail
        ? {
            ...currentDetail.thread,
            ...(pendingThreadSettingsRef.current ?? {}),
          }
        : null;
      const steerTargetTurnId =
        currentEffectiveThread?.status === 'running'
          ? currentEffectiveThread.activeTurnId
          : null;
      const shouldSteer =
        activeView === 'chat' && Boolean(steerTargetTurnId);

      if (shouldSteer && steerTargetTurnId) {
        setOptimisticSteers((current) => [
          ...current,
          {
            id: optimisticSteerId,
            clientRequestId,
            turnId: steerTargetTurnId,
            prompt: input.prompt,
            createdAt: optimisticStartedAt,
            status: 'steering',
          },
        ]);
      } else {
        optimisticAttachmentPreviews = buildOptimisticAttachmentPreviews(
          input.attachments,
        );
        clearBufferedLiveOutput();
        setLiveOutput('');
        setOptimisticTurn({
          id: optimisticTurnId,
          serverTurnId: null,
          startedAt: optimisticStartedAt,
          status: 'sending',
          error: null,
          prompt: input.prompt,
          attachmentPreviews: optimisticAttachmentPreviews,
          model: optimisticModel,
          reasoningEffort: optimisticReasoningEffort,
          reasoningEffortAvailable: getReasoningEffortAvailability(
            modelOptions,
            optimisticModel,
          ),
          tokenUsage: null,
          priceEstimate: null,
        });
      }

      const promptInput = {
        prompt: input.prompt,
        clientRequestId,
        ...(currentEffectiveThread?.model ? { model: currentEffectiveThread.model } : {}),
        ...(currentEffectiveThread?.reasoningEffort
          ? { reasoningEffort: currentEffectiveThread.reasoningEffort }
          : {}),
        ...(currentEffectiveThread?.collaborationMode
          ? { collaborationMode: currentEffectiveThread.collaborationMode }
          : {}),
        ...(input.attachments?.length ? { attachments: input.attachments } : {}),
      };
      const thread = await sendThreadPrompt(id, promptInput);
      if (activeRouteRef.current !== routeKey) return true;
      const nextThread =
        pendingThreadSettingsRef.current &&
        Object.keys(pendingThreadSettingsRef.current).length > 0
          ? {
              ...thread,
              ...pendingThreadSettingsRef.current,
            }
          : thread;
      setDetail((current) => (current ? { ...current, thread: nextThread } : current));
      setThreads((current) =>
        current.map((entry) => (entry.id === nextThread.id ? nextThread : entry)),
      );
      if (shouldSteer && steerTargetTurnId) {
        // The prompt response acknowledges persistence. Read only the queue if
        // its WebSocket receipt has not arrived; never wait for a history poll.
        if (!detailRef.current?.pendingSteers.some((item) => item.clientRequestId === clientRequestId)) {
          void fetchThreadDelivery(id).then((delivery) => {
            if (activeThreadIdRef.current !== id) return;
            setDetail((current) => current ? { ...current, pendingSteers: delivery.pendingSteers } : current);
            const acknowledged = new Set(delivery.pendingSteers.map((item) => item.clientRequestId));
            setOptimisticSteers((current) => current.filter((item) => !acknowledged.has(item.clientRequestId)));
          }).catch(() => { /* Realtime or the normal summary poll can recover. */ });
        }
        const fellBackToNewTurn =
          nextThread.activeTurnId !== null &&
          nextThread.activeTurnId !== steerTargetTurnId &&
          nextThread.lastTurnStartedAt !== currentEffectiveThread?.lastTurnStartedAt;

        if (fellBackToNewTurn) {
          optimisticAttachmentPreviews = buildOptimisticAttachmentPreviews(
            input.attachments,
          );
          clearBufferedLiveOutput();
          setLiveOutput('');
          setLivePlan(null);
          setOptimisticSteers((current) =>
            current.filter((steer) => steer.id !== optimisticSteerId),
          );
          setOptimisticTurn({
            id: optimisticTurnId,
            serverTurnId: nextThread.activeTurnId,
            startedAt: nextThread.lastTurnStartedAt ?? optimisticStartedAt,
            status: 'inProgress',
            error: null,
            prompt: input.prompt,
            attachmentPreviews: optimisticAttachmentPreviews,
            model: optimisticModel,
            reasoningEffort: optimisticReasoningEffort,
            reasoningEffortAvailable: getReasoningEffortAvailability(
              modelOptions,
              optimisticModel,
            ),
            tokenUsage: null,
            priceEstimate: null,
          });
        } else {
          setOptimisticSteers((current) =>
            current.map((steer) =>
              steer.id === optimisticSteerId
                ? {
                    ...steer,
                    turnId: nextThread.activeTurnId ?? steer.turnId,
                    status: 'accepted',
                  }
                : steer,
            ),
          );
        }
      } else {
        setOptimisticTurn((current) =>
          current && current.id === optimisticTurnId
            ? {
                ...current,
                id: nextThread.activeTurnId ?? current.id,
                serverTurnId: nextThread.activeTurnId ?? current.serverTurnId,
                status: 'inProgress',
                error: null,
                tokenUsage: current.tokenUsage,
                priceEstimate: current.priceEstimate,
              }
            : current,
        );
        setLivePlan(null);
      }
      if (input.delivery === 'steer' && shouldSteer && steerTargetTurnId) {
        try {
          await runDetailMutation(() => steerSubmittedPrompt(id, clientRequestId, steerTargetTurnId));
        } catch (caught) {
          // Acceptance succeeded. Keep the durable message and clear the draft,
          // rather than inviting a second submission of the same prompt.
          setError(translate("workbench.messageSavedButSteerCouldNotBe", { value1: caught instanceof Error ? caught.message : translate("workbench.tryTheQueuedMessageAfterReconnecting") }));
        }
      }
      setChatDraft({
        prompt: '',
        attachments: [],
      });
      return true;
    } catch (caught) {
      const message =
        caught instanceof ApiError
          ? caught.payload.message
          : caught instanceof Error
            ? caught.message
            : translate("workbench.unableToSendPrompt");
      if (caught instanceof ApiError) {
        setError(caught.payload.message);
      } else {
        setError(message);
      }
      setOptimisticSteers((current) =>
        current.filter((steer) => steer.clientRequestId !== clientRequestId),
      );
      setOptimisticTurn((current) =>
        current && current.id === optimisticTurnId
          ? {
              ...current,
              status: 'failed',
              error: message,
            }
          : current,
      );
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function handlePrompt(input: SendThreadPromptRequestInput) {
    if (activeRouteRef.current !== routeKey || promptSubmissionOwner.inFlight) return false;
    promptSubmissionOwner.inFlight = true;
    try {
      return await performPromptSubmission(input);
    } finally {
      promptSubmissionOwner.inFlight = false;
    }
  }

  async function ensureThreadConnectedForGoal() {
    const currentDetail = detailRef.current;
    if (!currentDetail) {
      setError(translate("workbench.threadDetailIsStillLoading"));
      return false;
    }

    if (currentDetail.thread.isLoaded) {
      return true;
    }

    setBusy(true);
    setError(null);

    try {
      const resumed = await resumeThread(
        id,
        {
          ...(currentDetail.thread.model ? { model: currentDetail.thread.model } : {}),
        },
      );
      setDetail((current) =>
        current
          ? {
              ...resumed,
              turns: appendLatestTurns(
                current.turns,
                resumed.turns,
                resumed.thread.activeTurnId,
              ),
            }
          : resumed,
      );
      setThreads((current) =>
        current.map((entry) =>
          entry.id === resumed.thread.id ? resumed.thread : entry,
        ),
      );
      return true;
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : translate("workbench.unableToConnectThisThreadBeforeSetting"),
      );
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function handleComposerGoalSubmit(input: {
    objective?: string | null;
    status?: NonNullable<ThreadDetailDto['goal']>['status'] | null;
    tokenBudget?: number | null;
  }) {
    const objective = input.objective?.trim() ?? '';
    const optimisticTurnId = objective
      ? `optimistic-goal-${Date.now()}`
      : null;
    const startedAt = new Date().toISOString();
    const currentDetail = detailRef.current;
    const optimisticThread = currentDetail?.thread ?? null;

    if (optimisticTurnId) {
      setScrollRequestKey((current) => current + 1);
      setOptimisticTurn({
        id: optimisticTurnId,
        serverTurnId: null,
        startedAt,
        status: 'sending',
        error: null,
        prompt: objective,
        attachmentPreviews: [],
        model: optimisticThread?.model ?? null,
        reasoningEffort: optimisticThread?.reasoningEffort ?? null,
        reasoningEffortAvailable: getReasoningEffortAvailability(
          modelOptions,
          optimisticThread?.model ?? null,
        ),
        tokenUsage: null,
        priceEstimate: null,
      });
    }

    try {
      await handleUpdateGoal(input);
      await loadThreadDetail({
        showLoading: false,
        clearError: false,
        reportError: false,
      });
      if (optimisticTurnId) {
        setOptimisticTurn((current) =>
          current && current.id === optimisticTurnId
            ? {
                ...current,
                status: 'completed',
                error: null,
              }
            : current,
        );
      }
    } catch (caught) {
      const message =
        caught instanceof ApiError
          ? caught.payload.message
          : caught instanceof Error
            ? caught.message
            : translate("workbench.unableToSetGoal");
      setError(message);
      if (optimisticTurnId) {
        setOptimisticTurn((current) =>
          current && current.id === optimisticTurnId
            ? {
                ...current,
                status: 'failed',
                error: message,
              }
            : current,
        );
      }
      throw caught;
    }
  }

  async function handleCopyMetaSessionId() {
    const sessionId = detail?.thread.providerSessionId;
    if (!sessionId) {
      return;
    }

    try {
      await navigator.clipboard.writeText(sessionId);
      setMetaSessionCopyState('copied');
    } catch {
      setMetaSessionCopyState('failed');
    }
  }

  async function handleThreadConnectionToggle(options?: { attachShell?: boolean }) {
    if (!detail) {
      return;
    }

    setBusy(true);
    setError(null);
    clearBufferedLiveOutput();
    setLiveOutput('');

    try {
      if (detail.thread.isLoaded && detail.thread.status !== 'recovering') {
        const disconnected = await disconnectThread(id);
        setDetail((current) =>
          current
            ? {
                ...disconnected,
                turns: appendLatestTurns(
                  current.turns,
                  disconnected.turns,
                  disconnected.thread.activeTurnId,
                ),
              }
            : disconnected,
        );
        setShellControlState(null);
        setThreads((current) =>
          current.map((entry) =>
            entry.id === disconnected.thread.id ? disconnected.thread : entry,
          ),
        );
        setPendingShellConnectionToggle(false);
        return;
      }

      const resumed = await resumeThread(
        id,
        {
          ...(detail.thread.model ? { model: detail.thread.model } : {}),
        },
      );
      setDetail((current) =>
        current
          ? {
              ...resumed,
              turns: appendLatestTurns(
                current.turns,
                resumed.turns,
                resumed.thread.activeTurnId,
              ),
            }
          : resumed,
      );
      setThreads((current) =>
        current.map((entry) =>
          entry.id === resumed.thread.id ? resumed.thread : entry,
        ),
      );
      if (options?.attachShell && activeView === 'shell') {
        setPendingShellConnectionToggle(true);
      }
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : translate("workbench.unableToChangeConnectionState"),
      );
    } finally {
        setBusy(false);
    }
  }

  async function handleInterrupt() {
    if (activeView === 'shell') {
      const sent = shellPanelRef.current?.sendControl('ctrl_c') ?? false;
      if (!sent) {
        setError(translate("workbench.connectTheShellBeforeSendingCtrlC"));
      } else {
        setError(null);
      }
      return;
    }

    setBusy(true);
    setError(null);
    interruptingRef.current = true;

    try {
      await runDetailMutation(async () => {
        if (detail?.thread.activeTurnId) {
          await interruptThread(id, { turnId: detail.thread.activeTurnId });
        } else {
          await interruptThread(id);
        }
        return fetchThreadDetail(id);
      });
      clearBufferedLiveOutput();
      setLiveOutput('');
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : translate("workbench.unableToInterruptTurn"),
      );
    } finally {
      interruptingRef.current = false;
      setBusy(false);
    }
  }

  async function handleUpdateThreadSettings(input: {
    model?: string;
    reasoningEffort?: ThreadDto['reasoningEffort'];
    fastMode?: boolean;
    collaborationMode?: ThreadDto['collaborationMode'];
    sandboxMode?: ThreadDto['sandboxMode'];
  }) {
    if (!detail) {
      return;
    }

    const previousDetail = detail;
    const mergedPendingThreadSettings: PendingThreadSettings = {
      ...(pendingThreadSettingsRef.current ?? {}),
      ...(input.model !== undefined ? { model: input.model } : {}),
      ...(input.reasoningEffort !== undefined
        ? { reasoningEffort: input.reasoningEffort }
        : {}),
      ...(input.fastMode !== undefined ? { fastMode: input.fastMode } : {}),
      ...(input.collaborationMode !== undefined
        ? { collaborationMode: input.collaborationMode }
        : {}),
      ...(input.sandboxMode !== undefined ? { sandboxMode: input.sandboxMode } : {}),
    };
    const optimisticThread = {
      ...detail.thread,
      ...mergedPendingThreadSettings,
    };

    setSettingsBusy(true);
    pendingThreadSettingsRef.current = mergedPendingThreadSettings;
    detailRef.current = {
      ...detail,
      thread: optimisticThread,
    };
    setDetail((current) =>
      current
        ? {
            ...current,
            thread: optimisticThread,
          }
        : current,
    );
    setThreads((current) =>
      current.map((entry) =>
        entry.id === optimisticThread.id ? { ...entry, ...optimisticThread } : entry,
      ),
    );

    try {
      const updated = await updateThreadSettings(id, {
        ...(input.model !== undefined ? { model: input.model } : {}),
        ...(input.reasoningEffort !== undefined
          ? { reasoningEffort: input.reasoningEffort }
          : {}),
        ...(input.fastMode !== undefined ? { fastMode: input.fastMode } : {}),
        ...(input.collaborationMode !== undefined
          ? { collaborationMode: input.collaborationMode }
          : {}),
        ...(input.sandboxMode !== undefined ? { sandboxMode: input.sandboxMode } : {}),
      });
      pendingThreadSettingsRef.current = null;
      detailRef.current = previousDetail
        ? {
            ...previousDetail,
            thread: updated,
          }
        : null;
      setDetail((current) =>
        current
          ? {
              ...current,
              thread: updated,
            }
          : current,
      );
      setThreads((current) =>
        current.map((entry) => (entry.id === updated.id ? updated : entry)),
      );
    } catch (caught) {
      pendingThreadSettingsRef.current = null;
      detailRef.current = previousDetail;
      setDetail(previousDetail);
      setThreads((current) =>
        current.map((entry) =>
          entry.id === previousDetail.thread.id ? previousDetail.thread : entry,
        ),
      );
      setError(
        caught instanceof Error
          ? caught.message
          : translate("workbench.unableToUpdateThreadSettings"),
      );
    } finally {
      setSettingsBusy(false);
    }
  }

  const handleRespondToRequest = useCallback(async (
    requestId: string,
    input: { answers: Record<string, { answers: string[] }> },
  ) => {
    if (!relayThreadCanControl) {
      setError(translate("workbench.thisSharedSessionIsViewOnly"));
      return;
    }
    setRespondingRequestId(requestId);
    setError(null);

    try {
      await runDetailMutation(async () => {
        const updated = await respondToThreadRequest(id, requestId, input);
        resolvedRequestIdsRef.current.add(requestId);
        return updated;
      });
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : translate("workbench.unableToAnswerThisRequest"),
      );
    } finally {
      setRespondingRequestId(null);
    }
  }, [id, relayThreadCanControl, runDetailMutation]);

  const handleLoadHistoryItemDetail = useCallback(
    (itemId: string) => fetchThreadHistoryItemDetail(id, itemId),
    [id],
  );

  const handleLoadTurnDetail = useCallback(
    (turnId: string) => fetchThreadTurnDetail(id, turnId),
    [id],
  );

  const handleCancelPendingSteer = useCallback(
    async (threadId: string, pendingSteerId: string) => {
      setError(null);
      try {
        await runDetailMutation(() =>
          cancelPendingSteer(threadId, pendingSteerId),
        );
      } catch (caught) {
        setError(
          caught instanceof Error
            ? caught.message
            : translate("workbench.unableToCancelQueuedPrompt"),
        );
        throw caught;
      }
    },
    [runDetailMutation],
  );

  const handleSteerPendingPrompt = useCallback(
    async (threadId: string, pendingSteerId: string) => {
      setError(null);
      try {
        await runDetailMutation(() =>
          steerPendingPrompt(threadId, pendingSteerId),
        );
      } catch (caught) {
        setError(
          caught instanceof Error
            ? caught.message
            : translate("workbench.unableToSteerQueuedPrompt"),
        );
        throw caught;
      }
    },
    [runDetailMutation],
  );

  async function handleCompactThread() {
    if (!detail) {
      return;
    }

    setCompactBusy(true);
    setError(null);

    try {
      const updated = await compactThread(id);
      setDetail((current) =>
        current
          ? {
              ...current,
              thread: updated,
            }
          : current,
      );
      setThreads((current) =>
        current.map((entry) => (entry.id === updated.id ? updated : entry)),
      );
    } catch (caught) {
      setError(
        caught instanceof Error
          ? caught.message
          : translate("workbench.unableToCompactThisThreadContext"),
      );
    } finally {
      setCompactBusy(false);
    }
  }

  async function handleRenameThread(threadId: string, title: string) {
    try {
      const updated = await updateThread(threadId, { title });
      setThreads((current) =>
        current.map((entry) =>
          entry.id === updated.id
            ? {
                ...entry,
                title: updated.title,
                updatedAt: updated.updatedAt,
              }
            : entry,
        ),
      );
      setDetail((current) =>
        current && current.thread.id === updated.id
          ? {
              ...current,
              thread: {
                ...current.thread,
                title: updated.title,
                updatedAt: updated.updatedAt,
              },
            }
          : current,
      );
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : translate("workbench.unableToRenameThread"));
      throw caught;
    }
  }

  async function handleDeleteThread() {
    if (!deletingThread) {
      return;
    }

    setDeletingThreadBusy(true);
    setError(null);
    try {
      await deleteThread(deletingThread.id);
      setThreads((current) =>
        current.filter((thread) => thread.id !== deletingThread.id),
      );
      const deletedCurrentThread = deletingThread.id === detail?.thread.id;
      setDeletingThread(null);
      if (deletedCurrentThread) {
        navigate(currentThreadsHref(deletingThread.workspaceId), { replace: true });
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : translate("workbench.unableToDeleteThread"));
    } finally {
      setDeletingThreadBusy(false);
    }
  }

  function handleToggleView() {
    toggleTerminal();
  }

  async function handleShellCopy() {
    const copied = await shellPanelRef.current?.copyLastCommandOutput();
    if (!copied) {
      setError(translate("workbench.unableToCopyTheLastShellCommand"));
    } else {
      setError(null);
    }
  }

  function handleShellControl(
    action: 'ctrl_c' | 'ctrl_d' | 'esc' | 'tab' | 'up' | 'down' | 'clear',
  ) {
    const sent =
      action === 'clear'
        ? (shellPanelRef.current?.sendCommand('clear') ?? false)
        : (shellPanelRef.current?.sendControl(action) ?? false);
    if (!sent) {
      setError(translate("workbench.connectTheShellBeforeSendingControlInput"));
    } else {
      setError(null);
    }
  }

  useEffect(() => {
    if (
      !pendingShellConnectionToggle ||
      activeView !== 'shell' ||
      !shellPanelRef.current ||
      detail?.thread.isLoaded === false ||
      shellControlState?.loading !== false
    ) {
      return;
    }

    if (shellControlState?.status === 'attached') {
      setPendingShellConnectionToggle(false);
      return;
    }

    setPendingShellConnectionToggle(false);
    void shellPanelRef.current.toggleConnection();
  }, [
    activeView,
    detail?.thread.isLoaded,
    pendingShellConnectionToggle,
    shellControlState?.loading,
    shellControlState?.status,
  ]);

  useEffect(() => {
    if (activeView !== 'shell') {
      return;
    }

    const frame = window.requestAnimationFrame(() => {
      shellPanelRef.current?.refreshLayout({ syncBackendSize: false });
    });

    return () => {
      window.cancelAnimationFrame(frame);
    };
  }, [activeView]);

  const promptDisabledReason = detail
    ? detail.workspacePathStatus === 'missing'
      ? translate("workbench.restoreThisWorkspacePathOnTheCurrent")
      : relayDeviceRouteActive && relayAccessState.status === 'loading'
        ? translate("workbench.checkingRelayPermissions")
      : relayDeviceRouteActive && relayAccessState.status === 'failed'
        ? relayAccessState.error ?? translate("workbench.unableToVerifyRelayPermissions")
      : relayAccess?.kind === 'shared' && relayAccess.threadAccess === 'read'
        ? translate("workbench.thisSharedSessionIsViewOnly")
      : !relayThreadCanControl
        ? translate("workbench.checkingRelayPermissions")
      : null
    : null;
  // The workbench composer sits inside the chat panel, which reserves its own
  // scroll space; a second timeline spacer would hide replies in a short chat
  // (for example above an open terminal).
  const { floatingMobileComposerBottomOffset } = useMobileComposerLayout({
    activeView,
    composerHostRef,
    threadId: detail?.thread.id ?? id,
  });

  const metaContent = detail ? (
    <dl className="space-y-4 text-sm">
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.pockymoeSessionID")}</dt>
        <dd className="mt-1 break-all text-[var(--theme-fg)]">{detail.thread.id}</dd>
      </div>
      <div className="relative pr-9">
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.harnessSessionID")}</dt>
        <dd className="mt-1 break-all text-[var(--theme-fg)]">
          {detail.thread.providerSessionId ?? translate("workbench.unavailable")}
        </dd>
        {(detail.thread.providerSessionId) && (
          <button
            type="button"
            aria-label={translate("workbench.copyHarnessSessionID")}
            title={
              metaSessionCopyState === 'copied'
                ? translate("workbench.copied")
                : metaSessionCopyState === 'failed'
                  ? translate("workbench.copyFailed")
                  : translate("workbench.copyHarnessSessionID")
            }
            onClick={() => void handleCopyMetaSessionId()}
            className={`thread-mobile-hit-target absolute bottom-0 right-0 inline-flex h-5 w-5 items-center justify-center rounded-full border shadow-sm backdrop-blur transition ${
              metaSessionCopyState === 'copied'
                ? 'ui-status-info'
                : metaSessionCopyState === 'failed'
                  ? 'ui-status-danger'
                  : 'border-[var(--theme-border)] bg-[var(--theme-surface-strong)] text-[var(--theme-fg-soft)] hover:bg-[var(--theme-hover)] hover:text-[var(--theme-fg)]'
            }`}
          >
            <span className="scale-[0.72]">
              <CopyIcon />
            </span>
          </button>
        )}
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.source")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.thread.source === 'supervisor'
            ? translate("workbench.supervisorThread", { value1: detail.thread.provider })
            : translate("workbench.importedLocalSession", { value1: detail.thread.provider })}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.status")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {threadStatusLabel(detail.thread.status)}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.created")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {formatLongTimestamp(detail.thread.createdAt)}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.workspace")}</dt>
        <dd className="mt-1 break-words text-[var(--theme-fg)]">
          {detail.workspace.absPath}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.workspacePath")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.workspacePathStatus === 'present' ? translate("workbench.present") : translate("workbench.missingOnThisMachine")}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.activeTurn")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.thread.activeTurnId ?? translate("workbench.none")}
        </dd>
      </div>
    </dl>
  ) : null;

  const settingsContent = toolsDetail ? <div data-testid="workbench-session-settings" data-device={toolsDevice} data-thread={toolsDetail.thread.id}>
    <p className="mb-3 text-sm text-[var(--theme-fg-muted)]">{toolsDetail.workspace.label} · {toolsDetail.thread.title}</p>
    <HarnessSettingsFields key={`${toolsDevice}:${toolsDetail.thread.id}`} thread={toolsDetail.thread}
      models={toolsUseReference ? referenceController.models : modelOptions}
      busy={toolsUseReference ? referenceController.busy : settingsBusy} readOnly={!toolsCanControl}
      onChange={async input => { if (toolsUseReference) await referenceController.updateSettings(input); else await handleUpdateThreadSettings(input); }}
      loadCapabilities={loadToolsCapabilities}
      {...harnessPanel(toolsDetail.thread.id, toolsDevice)} harnessReadOnly={!toolsHarnessOwner} />
  </div> : null;
  const dialogUseReference = harnessSettingsTarget?.pane === 'reference';
  const dialogDetail = dialogUseReference ? referenceController.detail : detail;
  const dialogDevice = dialogUseReference ? referenceDevice : (relayRouteDeviceId ?? 'local');
  const settingsDialogVisible = Boolean(harnessSettingsTarget && dialogDetail?.thread.id === harnessSettingsTarget.threadId && dialogDevice === harnessSettingsTarget.deviceId);

  const optimisticMaterializedTurn =
    optimisticTurn && detail
      ? findMaterializedOptimisticTurn(detail.turns, optimisticTurn)
      : null;
  const timelineOptimisticTurn = useMemo(
    () =>
      optimisticTurn && !optimisticMaterializedTurn
        ? {
            id: optimisticTurn.id,
            startedAt: optimisticTurn.startedAt,
            status: optimisticTurn.status,
            error: optimisticTurn.error,
            model: optimisticTurn.model,
            reasoningEffort: optimisticTurn.reasoningEffort,
            reasoningEffortAvailable: optimisticTurn.reasoningEffortAvailable,
            tokenUsage: optimisticTurn.tokenUsage,
            priceEstimate: optimisticTurn.priceEstimate,
            items: [
              {
                id: `${optimisticTurn.id}-user-message`,
                kind: 'userMessage' as const,
                text: optimisticTurn.prompt,
                attachmentPreviewUrls: Object.fromEntries(
                  optimisticTurn.attachmentPreviews.map((preview) => [
                    preview.path,
                    preview.url,
                  ]),
                ),
              },
            ],
          }
        : null,
    [optimisticMaterializedTurn, optimisticTurn],
  );

  const threadLoaded = detail?.thread.isLoaded ?? false;
  const realtimeConnectionLabel = threadConnectionSummary(threadLoaded, realtimeConnection);
  const sessionConnectionIndicator = (
    <DeviceEncryptionStatus hideHealthy deviceId={relayRouteDeviceId ?? undefined} connection={{
      loaded: threadLoaded && detail?.thread.status !== 'recovering',
      busy: busy || !detail,
      state: realtimeConnection.status,
      label: detail?.thread.status === 'recovering' ? translate("workbench.statusUnconfirmedReconnectToVerify") : realtimeConnectionLabel,
      onConnect: () => void handleThreadConnectionToggle(),
    }} />
  );
  const mobileSessionConnectionControl = sessionConnectionIndicator;
  const currentGoal = goalState.data ?? detail?.goal ?? null;
  const goalHistory = detail?.goalHistory ?? [];
  const monitorGoals = currentGoal
    ? mergeGoalHistory(goalHistory, currentGoal)
    : normalizeGoalHistory(goalHistory);
  const relayAccessBadge = useMemo(
    () =>
      relayAccess?.kind === 'shared' ? (
        <div
          className="host-secondary-button inline-flex max-w-[10rem] items-center gap-1.5 rounded-full border px-2.5 py-1 text-[11px] font-medium sm:max-w-[16rem]"
          title={`${relayThreadAccessLabel(relayAccess.threadAccess)} / ${relayWorkspaceAccessLabel(relayAccess.workspaceAccess)}`}
        >
          <span>{relayThreadAccessLabel(relayAccess.threadAccess)}</span>
          <span className="host-muted">/</span>
          <span className="truncate">
            {relayWorkspaceAccessLabel(relayAccess.workspaceAccess)}
          </span>
        </div>
      ) : null,
    [relayAccess, i18nLocale],
  );
  const threadActionsButton = <div>
    {relayThreadIsOwner && relayRouteDeviceId && <HostFeatureGate feature="previews"><button aria-label={translate("workbench.portMappings")} title={translate("workbench.portMappings")} onClick={() => setPortsOpen(true)}><Network data-icon="ports" /></button></HostFeatureGate>}
    {!hostScope && <button aria-label={translate("workbench.shareAsLink")} title={translate("workbench.createAndCopyReadOnlyLink")} onClick={() => { setActionMode('link'); setExportDialogOpen(true); }}><Link2 data-icon="share-link" /></button>}
    {!hostScope && <button aria-label={translate("workbench.sharingPermissions")} title={translate("workbench.sharingPermissions")} onClick={() => { setActionMode('share'); setExportDialogOpen(true); }}><Users data-icon="sharing" /></button>}
    <button aria-label={translate("workbench.downloadTranscript")} title={translate("workbench.downloadTranscript")} disabled={!detail} onClick={() => { setActionMode('html'); setExportDialogOpen(true); }}><Download data-icon="download" /></button>
  </div>;
  const mobileSessionConnectionButton = useMemo(
    () => (
      <div className="relative flex items-center justify-end gap-1.5">
        {relayAccessBadge}
        {mobileSessionConnectionControl}
      </div>
    ),
    [mobileSessionConnectionControl, relayAccessBadge],
  );
  const surfaceActions = useMemo(
    () => (
      <div className="flex items-center justify-end gap-2">
        {relayAccessBadge}
        {sessionConnectionIndicator}
      </div>
    ),
    [sessionConnectionIndicator, relayAccessBadge, relayRouteDeviceId],
  );
  const timelineProps = useMemo<Partial<ThreadTimelineProps>>(
    () => ({
      livePlan,
      liveItems,
      backendProgress: backendProgress?.threadId === id ? backendProgress : null,
      backgroundAgentCount: detail?.activeSubagents?.filter((agent) => agent.isBackground && agent.status === 'running').length ?? 0,
      respondingRequestId,
      ...(relayThreadCanControl ? { onRespondToRequest: handleRespondToRequest } : {}),
      scrollRequestKey,
      previousTurnScrollRequestKey,
      nextTurnScrollRequestKey,
      ...(searchTarget ? { searchTarget } : {}),
      className: 'thread-timeline-surface min-h-0 flex-1',
      onTailVisibilityChange: setFollowTail,
      onPreviousTurnAvailabilityChange: setCanJumpToPreviousTurn,
      onNextTurnAvailabilityChange: setCanJumpToNextTurn,
      loadingEarlier,
      onLoadEarlier: handleLoadEarlierTurns,
      onOpenThread: openThread,
      answeredRequestNotes:
        detail?.answeredRequestNotes ?? EMPTY_ANSWERED_REQUEST_NOTES,
      activityNotes: detail?.activityNotes ?? EMPTY_ACTIVITY_NOTES,
      pendingSteers: detail?.pendingSteers ?? EMPTY_PENDING_STEERS,
      optimisticSteers,
      optimisticTurn: timelineOptimisticTurn,
    }),
    [
      detail?.answeredRequestNotes,
      detail?.activityNotes,
      detail?.pendingSteers,
      detail?.activeSubagents,
      handleLoadEarlierTurns,
      handleRespondToRequest,
      relayThreadCanControl,
      liveItems,
      livePlan,
      backendProgress,
      id,
      loadingEarlier,
      openThread,
      optimisticSteers,
      respondingRequestId,
      scrollRequestKey,
      previousTurnScrollRequestKey,
      nextTurnScrollRequestKey,
      searchTarget,
      timelineOptimisticTurn,
    ],
  );
  const chatComposerProps = detail
    ? ({
        sendShortcut: shellNav?.sendShortcut ?? 'ctrlEnter',
        busy: activeView === 'chat' ? busy : false,
        settingsBusy,
        error: null,
        model: detail.thread.model,
        agentLabel:
          agentOptions.find((entry) => entry.model === detail.thread.agentId)?.displayName ??
          detail.thread.agentId ?? null,
        reasoningEffort: detail.thread.reasoningEffort,
        fastMode: detail.thread.fastMode ?? false,
        collaborationMode: detail.thread.collaborationMode,
        sandboxMode: detail.thread.sandboxMode ?? null,
        modelOptions,
        contextUsage: resolveThreadContextUsage(detail),
        subscriptionUsage,
        capabilities: hostScope && backendCapabilities ? { ...backendCapabilities, branching: { ...backendCapabilities.branching, fork: false, forkAt: false, resumeAt: false }, management: { ...backendCapabilities.management, hooks: false, hostConfigFiles: false } } : backendCapabilities,
        toolboxItems: hostScope ? [] : backendManagementSchema?.toolboxItems ?? [],
        onOpenHarness: (composerFocusOwner?: string) => setHarnessSettingsTarget({ pane: 'primary', deviceId: relayRouteDeviceId ?? 'local', threadId: id, ...(composerFocusOwner ? {composerFocusOwner} : {}) }),
        hookCommandTemplates:
          backendManagementSchema?.hookCommandTemplates ?? [],
        mcpConfigFormat: backendManagementSchema?.mcpConfigFormat ?? 'none',
        followTail,
        threadConnected: detail.thread.isLoaded,
        shellAvailable: terminalPluginEnabled,
        disabled: Boolean(promptDisabledReason),
        ...(promptDisabledReason
          ? { disabledPlaceholder: promptDisabledReason }
          : {}),
        shellControlState,
        draftPrompt: chatDraft.prompt,
        draftAttachments: chatDraft.attachments,
        onDraftChange: setChatDraft,
        canInterrupt: Boolean(
          detail.thread.activeTurnId && relayThreadCanControl,
        ),
        ...(relayThreadCanControl ? { onInterrupt: handleInterrupt } : {}),
        ...(relayThreadCanControl
          ? {
              onCompact: handleCompactThread,
              ...(!hostScope ? {
                onOpenForkTurns: handleOpenForkTurns,
                onForkLatest: handleForkLatest,
                onForkTurn: handleForkTurn,
              } : {}),
              onOpenSkills: handleOpenSkills,
              onOpenMcp: handleOpenMcp,
              ...(!hostScope ? { onOpenHooks: handleOpenHooks } : {}),
              ...(!hostScope ? {
                onCreateHook: handleCreateHook,
                onUpdateHook: handleUpdateHook,
                onTrustHook: handleTrustHook,
                onUntrustHook: handleUntrustHook,
              } : {}),
            }
          : {}),
        goalState,
        goalHistory: monitorGoals,
        ...(relayThreadCanControl
          ? {
              onOpenGoal: handleOpenGoal,
              onPrepareGoalSubmit: ensureThreadConnectedForGoal,
              onUpdateGoal: handleComposerGoalSubmit,
            }
          : {}),
        ...(mcpProviderConfigFileName
          ? {
              ...(relayThreadIsOwner
                ? {
                    onReadProviderConfig: () =>
                      fetchProviderHostFile(
                        detail.thread.provider,
                        mcpProviderConfigFileName,
                      ),
                    onWriteProviderConfig: (content: string) =>
                      updateProviderHostFile(
                        detail.thread.provider,
                        mcpProviderConfigFileName,
                        { content },
                      ),
                  }
                : {}),
            }
          : {}),
        onToggleFollow: () => setScrollRequestKey((current) => current + 1),
        canJumpToPreviousTurn,
        onJumpToPreviousTurn: () =>
          setPreviousTurnScrollRequestKey((current) => current + 1),
        canJumpToNextTurn,
        onJumpToNextTurn: () =>
          setNextTurnScrollRequestKey((current) => current + 1),
        ...(relayThreadCanControl
          ? { onUpdateSettings: handleUpdateThreadSettings }
          : {}),
        onToggleView: handleToggleView,
        onShellCopy: handleShellCopy,
        ...(relayThreadCanControl ? { onShellControl: handleShellControl } : {}),
        compactBusy,
        skillsState,
        mcpState,
        hooksState,
        forkTurnOptionsState,
      } satisfies Omit<ThreadComposerProps, 'activeView' | 'onSubmit'>)
    : null;
  const shellComposerProps = detail
    ? ({
        busy,
        settingsBusy: false,
        error: detail.thread.isLoaded ? shellControlState?.error ?? null : null,
        followTail: false,
        capabilities: hostScope && backendCapabilities ? { ...backendCapabilities, branching: { ...backendCapabilities.branching, fork: false, forkAt: false, resumeAt: false }, management: { ...backendCapabilities.management, hooks: false, hostConfigFiles: false } } : backendCapabilities,
        toolboxItems: hostScope ? [] : backendManagementSchema?.toolboxItems ?? [],
        onOpenHarness: (composerFocusOwner?: string) => setHarnessSettingsTarget({ pane: 'primary', deviceId: relayRouteDeviceId ?? 'local', threadId: id, ...(composerFocusOwner ? {composerFocusOwner} : {}) }),
        hookCommandTemplates:
          backendManagementSchema?.hookCommandTemplates ?? [],
        mcpConfigFormat: backendManagementSchema?.mcpConfigFormat ?? 'none',
        threadConnected: detail.thread.isLoaded,
        shellAvailable: terminalPluginEnabled,
        shellControlState,
        canInterrupt: Boolean(
          detail.thread.isLoaded &&
            shellControlState?.isCommandRunning &&
            relayThreadCanControl,
        ),
        ...(relayThreadCanControl ? { onInterrupt: handleInterrupt } : {}),
        onToggleView: handleToggleView,
        onShellCopy: handleShellCopy,
        ...(relayThreadCanControl ? { onShellControl: handleShellControl } : {}),
      } satisfies Omit<ThreadComposerProps, 'activeView' | 'onSubmit'>)
    : null;
  const getCurrentThreadImageAssetUrl = useCallback(
    (path: string, variant?: 'thumbnail') =>
      detail
        ? getThreadImageAssetUrl({ threadId: detail.thread.id, path, ...(variant ? { variant } : {}) })
        : '',
    [detail?.thread.id, getThreadImageAssetUrl],
  );
  const primaryWorkspaceAdapter = useThreadWorkspaceAdapter({ setError, workspaceId: detail?.workspace.id ?? null, deviceId: relayRouteDeviceId ?? 'local', access: effectiveWorkspaceAccess, allowLinkedFiles: relayThreadIsOwner });
  const workspaceAdapter = useThreadWorkspaceAdapter({
    setError,
    workspaceId: toolsDetail?.workspace.id ?? null,
    deviceId: toolsDevice,
    access: hostScope ? 'read' : toolsWorkspaceAccess,
    allowLinkedFiles: !hostScope && (!toolsRelay || toolsAccess?.kind === 'owner'),
  });
  const handleOpenWorkspaceFile = useCallback(
    (input: { path: string; line?: number }) => {
      const currentDetail = detailRef.current;
      if (!currentDetail) return;
      if (toolsUseReference && !confirmWorkspaceDocumentLeave()) return;
      setFocusedPane('primary');

      const relativePath = relativeWorkspaceLinkPath(
        input.path,
        currentDetail.workspace.absPath,
      );
      setActiveView('chat');
      setWorkspaceFocusPathRequest((current) => ({
        path: relativePath ?? input.path,
        ...(input.line !== undefined ? { line: input.line } : {}),
        requestId: (current?.requestId ?? 0) + 1,
      }));
    },
    [toolsUseReference, setFocusedPane],
  );
  const openReferenceFile = useCallback((input: {path: string; line?: number}) => {
    if (!referenceController.detail) return;
    if (!toolsUseReference && !confirmWorkspaceDocumentLeave()) return;
    setFocusedPane('reference');
    const path = relativeWorkspaceLinkPath(input.path, referenceController.detail.workspace.absPath) ?? input.path;
    setWorkspaceFocusPathRequest(current => ({ path, ...(input.line !== undefined ? {line:input.line} : {}), requestId: (current?.requestId ?? 0)+1 }));
  }, [referenceController.detail, toolsUseReference, setFocusedPane]);
  const surfaceAdapter = useMemo(
    () => ({
      openThread,
      getThreadHref,
      getNewThreadHref,
      renderNewThreadDialogContent,
      ...(relayThreadIsOwner || hostScope ? { renameThread: handleRenameThread } : {}),
      ...(relayThreadIsOwner || hostScope ? { deleteThread: setDeletingThread } : {}),
      ...(relayThreadCanControl ? { cancelPendingSteer: handleCancelPendingSteer } : {}),
      ...(relayThreadCanControl
        ? { steerPendingPrompt: handleSteerPendingPrompt }
        : {}),
      sendPrompt: handlePrompt,
      ...(relayThreadCanControl ? { interrupt: handleInterrupt } : {}),
      ...(relayThreadCanControl ? { compact: handleCompactThread } : {}),
      ...(relayThreadCanControl
        ? { updateSettings: handleUpdateThreadSettings }
        : {}),
      loadHistoryItemDetail: handleLoadHistoryItemDetail,
      loadTurnDetail: handleLoadTurnDetail,
      getImageAssetUrl: getCurrentThreadImageAssetUrl,
      openWorkspaceFile: handleOpenWorkspaceFile,
      workspace: primaryWorkspaceAdapter,
      shell: localShellAdapter,
    }),
    [
      getCurrentThreadImageAssetUrl,
      getNewThreadHref,
      getThreadHref,
      renderNewThreadDialogContent,
      handleCompactThread,
      handleCancelPendingSteer,
      handleSteerPendingPrompt,
      handleInterrupt,
      handleLoadHistoryItemDetail,
      handleLoadTurnDetail,
      handleOpenWorkspaceFile,
      handlePrompt,
      handleRenameThread,
      handleUpdateThreadSettings,
      localShellAdapter,
      openThread,
      relayThreadCanControl,
      relayThreadIsOwner,
      hostScope,
      primaryWorkspaceAdapter,
    ],
  );
  const workspaceReturnHref = appUrl(currentWorkspacesHref());
  // A failed turn's error sits behind a mark on its prompt, not again above the thread.
  const pageError = loading ? null : error ?? detail?.thread.lastError ?? null;
  const errorOnPrompt = pageError !== null && (
    (optimisticTurn?.status === 'failed' && optimisticTurn.error === pageError)
    || Boolean(detail?.turns.some(turn => turn.status === 'failed' && turn.error === pageError)));
  const dialogs = useMemo(
    () => (
      <>
        <ThreadActionsDialog
          appearance="matter"
          initialMode={actionMode}
          linkContent={relayThreadCanShare && relayRouteDeviceId && id
            ? <ThreadPublicLinks deviceId={relayRouteDeviceId} threadId={id} />
            : <p className="matter-sharing-unavailable" role="status"><Link2 size={20} />{relayDeviceRouteActive ? translate("workbench.onlyTheOwnerCanCreateAPublic") : translate("workbench.openThisDeviceThroughYourRelayAccount")}</p>}
          open={exportDialogOpen}
          busy={exportBusy || shareBusy}
          turnsState={exportTurnsState}
          shareAvailable={relayThreadCanShare}
          shareUnavailableMessage={relayDeviceRouteActive ? translate("workbench.onlyTheOwnerCanShareThisSession") : translate("workbench.openThisDeviceThroughYourRelayAccount_7bd037")}
          shareState={threadShareState}
          onCancel={() => {
            if (!exportBusy && !shareBusy) {
              setExportDialogOpen(false);
            }
          }}
          onLoadTurns={loadExportTurns}
          onExport={handleExportTranscript}
          {...(relayThreadCanShare
            ? {
                onCreateShare: handleCreateThreadShare,
                onUpdateShare: handleUpdateThreadShare,
                deviceShareAvailable: true,
                onRevokeShare: handleRevokeThreadShare,
              }
            : {})}
        />
        <ConfirmDialog
          open={deletingThread !== null}
          title={translate("workbench.deleteThread_114337")}
          description={
            deletingThread
              ? translate("workbench.deleteFromSupervisorTheBackendSessionId", { value1: truncateAutoThreadTitle(deletingThread.title) })
              : ''
          }
          confirmLabel={translate("workbench.deleteThread_114337")}
          busy={deletingThreadBusy}
          onCancel={() => {
            if (!deletingThreadBusy) {
              setDeletingThread(null);
            }
          }}
          onConfirm={() => void handleDeleteThread()}
        />
      </>
    ),
    [
      actionMode,
      deletingThread,
      deletingThreadBusy,
      exportBusy,
      exportDialogOpen,
      exportTurnsState,
      handleCreateThreadShare,
      handleDeleteThread,
      handleExportTranscript,
      handleRevokeThreadShare,
      loadExportTurns,
      relayDeviceRouteActive,
      shareBusy,
      handleUpdateThreadShare, relayRouteDeviceId, id,
      threadShareState,
    , i18nLocale],
  );

  return (
    <>
    {nativeResult && <LongTextDialog open title={nativeResult.title} text={nativeResult.text} onClose={() => setNativeResult(null)} />}
    {relayThreadIsOwner && relayRouteDeviceId && <HostFeatureGate feature="previews"><PortMappingsControl key={relayRouteDeviceId} deviceId={relayRouteDeviceId} open={portsOpen} onOpenChange={setPortsOpen} /></HostFeatureGate>}
    <ThreadDetailSurface
      restrictedNavigation={Boolean(hostScope)}
      deviceMonitor={hostScope ? null : <><DeviceMonitor key={relayRouteDeviceId ?? 'local'} /><TourLauncherButton /></>}
      workbench={workbenchUrls({ ...workbenchNavigation, panels: {
        deviceLabel: presentationDeviceName ?? (relayRouteDeviceId ? relayRouteDeviceId.slice(0, 8) : translate('workbench.localDeviceName')),
        workspaceLabel: detail?.workspace.label ?? workbenchNavigation.workspacePath,
        primaryTitle: detail?.thread.title ?? translate('workbench.loadingThreadDetail'),
        primaryHarness: detail?.thread.agentId ?? detail?.thread.provider ?? '',
        primaryStatus: detail ? threadStatusLabel(detail.thread.status) : '',
        // Status marks for the phone's two pinned conversations, as the sidebar shows them.
        marks: {
          primary: [...workbenchNavigation.workspaceThreads, ...workbenchNavigation.threads].find(thread => thread.key === workbenchNavigation.currentKey)?.status,
          reference: referenceId ? [...workbenchNavigation.workspaceThreads, ...workbenchNavigation.threads].find(thread => thread.key === `${referenceDevice ?? 'local'}:${referenceId}`)?.status : undefined,
        },
        presentation: !referenceAllowed ? { ...workbenchPresentation.value, mode: 'focus' } : workbenchPresentation.value.referenceId === id && referenceDevice === (relayRouteDeviceId ?? 'local') ? { ...workbenchPresentation.value, mode: workbenchPresentation.value.mode === 'thread' ? 'focus' : workbenchPresentation.value.mode } : workbenchPresentation.value,
        onPresentationChange: changePresentation,
        candidates: threads.filter(thread => thread.id !== id).map(thread => ({ id: thread.id, title: thread.title })),
        referenceTitle: referenceController.detail?.thread.title ?? threads.find(thread => thread.id === referenceId)?.title ?? translate('workbench.loadingThreadDetail'),
        referenceContent: referenceId ? <ThreadPluginScope key={`${referenceDevice}:${referenceId}`} threadId={referenceId} adapter={referencePluginAdapter}><WorkbenchReferencePane key={`${referenceDevice}:${referenceId}`} controller={referenceController} onOpenThread={openReferenceThread} onOpenWorkspaceFile={openReferenceFile} onOpenHarness={composerFocusOwner => setHarnessSettingsTarget({pane:'reference',deviceId:referenceDevice,threadId:referenceId,...(composerFocusOwner ? {composerFocusOwner} : {})})} draft={referenceDraft} onDraftChange={setReferenceDraft} sendShortcut={shellNav?.sendShortcut ?? 'ctrlEnter'} /></ThreadPluginScope> : null,
        collaborationContent: <WorkbenchCollaboration detail={detail} threads={threads} onCompare={onCompareThread} onOpen={openThread} sourceKey={routeKey} visible={workbenchPresentation.value.mode === 'collaboration'} liveItems={liveItems} onNativeResult={target => {
          const source = routeKey;
          void fetchThreadTurnDetail(id, target.turnId).then(async turn => {
            if (activeRouteRef.current !== source) return;
            setDetail(current => current ? { ...current, turns: prependTurns(current.turns.map(existing => existing.id === turn.id ? turn : existing), [turn]) } : current);
            const item = turn.items.find(item => item.id === target.itemId);
            // Compact turn details defer tool output to the item-detail endpoint.
            const text = item?.hasDeferredDetail ? (await fetchThreadHistoryItemDetail(id, item.id)).text : item?.detailText;
            if (activeRouteRef.current !== source) return;
            if (item) setNativeResult({ title: item.text, text: text ?? item.text });
            workbenchPresentation.update({ mode: 'focus' });
            setSearchTarget({ ...target, key: Date.now() });
          }).catch(caught => { if (activeRouteRef.current === source) setError(actionErrorMessage(caught, translate('workbench.statusUnavailable'))); });
        }} />,
        onMakePrimary: makeReferencePrimary,
        focusedPane: toolsUseReference ? 'reference' : 'primary',
        onFocusPane: focusPane,
        toolsOpen: terminalOpen || dshPanelOpen,
        toolsTargetLabel,
        inlineFilesHeader: true,
        toolTitle: `${translate(dshPanelOpen ? 'workbench.deepseekHarness' : 'workbench.terminal')} · ${toolsDetail?.workspace.label ?? ''} · ${toolsDetail?.thread.title ?? ''}`,
        toolContent: controls => <>
          <WorkbenchTerminal deviceId={toolsDevice} detail={toolsDetail} canControl={toolsCanControl} open={terminalOpen} controls={controls} onClose={closeTerminal} effectiveTheme={shellNav?.effectiveTheme ?? 'dark'} targetLabel={toolsTargetLabel} openRequest={terminalOpenRequest} />
          {dshPanelOpen && dshPanelProps ? <DshPluginPanel key={`${toolsDevice}:${toolsDetail?.thread.id ?? ''}`} loadCapabilities={loadToolsCapabilities} readOnly={!toolsHarnessOwner} {...dshPanelProps} /> : null}
        </>,
        onCloseTools: closeTerminal,
        storageFailed: workbenchPresentation.storageFailed,
      }, subscriptionUsage, statusActions: detail ? <><ThreadSubagentsControl key={`subagents-${detail.thread.id}`} detail={detail} /><UnscopedOnly><ThreadWatchesControl key={`watches-${detail.thread.id}`} thread={detail.thread} /></UnscopedOnly><ThreadPluginsControl key={`plugins-${detail.thread.id}`} readOnly={!relayThreadCanControl} /><WorkbenchThreadPicker deviceId={relayRouteDeviceId} workspaceId={detail.workspace.id} threadId={detail.thread.id} threads={threads} navigationThreads={[...workbenchNavigation.workspaceThreads, ...workbenchNavigation.threads]} reference={referenceId ? {deviceId: referenceDevice === 'local' ? null : referenceDevice, threadId:referenceId} : null} splitActive={Boolean(referenceId && ['thread','files'].includes(workbenchPresentation.value.mode))} onSelect={selectSplitThread} onRestore={() => changePresentation({mode:'thread'})} onClose={() => changePresentation({mode:'focus'})} onCollaboration={() => changePresentation({mode:'collaboration'})} /></> : null, renderThreadMenu: thread => <RecentThreadMenu thread={thread} currentKey={workbenchNavigation.currentKey} onFavorite={workbenchNavigation.onToggleThreadFavorite} onRenamed={workbenchNavigation.onThreadRenamed} onRemoved={workbenchNavigation.onThreadRemoved} onNavigate={navigate} />, harnessSessionId: detail?.thread.providerSessionId ?? null, harnessSessionUrl: detail?.thread.providerSessionId && (detail.thread.provider === 'codex' || detail.thread.agentId === 'codex') ? `codex://threads/${encodeURIComponent(detail.thread.providerSessionId)}` : null, activeView: terminalOpen ? 'shell' : 'chat', terminalEnabled: terminalPluginEnabled, onViewChange: view => { if (view === 'shell') toggleTerminal(); },
        toolPanels: dshPanelAvailable ? [{ id: 'deepseek-harness', label: translate('workbench.deepseekHarness'), icon: <Puzzle data-icon="harness-panel" />, active: dshPanelOpen, onToggle: toggleDshPanel }] : [], onNavigate: navigate, onPrefetchThread: prefetchThread, onSearch: () => setSearchOpen(true), searchOpen, search: id && detail ? <ConversationSearch key={id} threadId={id} workspaceId={detail.thread.workspaceId}
        deviceLabel={relayRouteDeviceId ?? searchLabels.localDevice}
        allowGlobal={!hostScope && (relayThreadIsOwner || (relayAccess?.scope === 'device' && (relayAccess.threadAccess === 'read' || relayAccess.threadAccess === 'control')))}
        onNavigate={(match, query) => {
          const params = new URLSearchParams();
          if (match.turnId && match.itemId) { params.set('searchTurn', match.turnId); params.set('searchItem', match.itemId); if (query) params.set('searchQuery', query); }
          navigate(`${currentThreadHref(match.threadId)}${params.size ? `?${params}` : ''}`);
        }} turns={detail.turns} open={searchOpen}
        onOpen={() => setSearchOpen(true)} onClose={() => setSearchOpen(false)}
        onSelect={(turns, turnId, itemId, query) => {
          setDetail(current => current ? { ...current, turns: prependTurns(current.turns.map(turn => {
            const full = turns.find(t => t.id === turn.id);
            return full && turn.status !== 'inProgress' ? full : turn;
          }), turns) } : current);
          setSearchTarget({ turnId, itemId, key: Date.now(), ...(query ? { query } : {}) });
        }} /> : undefined, })}
      threads={threads}
      detail={detail}
      status={status}
      loading={loading}
      error={errorOnPrompt ? null : pageError}
      plugins={plugins}
      adapter={surfaceAdapter}
      metaContent={metaContent}
      settingsContent={settingsContent}
      settingsSections={appSettingsSections()}
      mobileHeaderAction={mobileSessionConnectionButton}
      workspaceReturnHref={workspaceReturnHref}
      onCloseAppNavigation={shellNav?.closeNav ?? (() => {})}
      threadActionsButton={threadActionsButton}
      surfaceActions={surfaceActions}
      workspaceFeatures={SUPERVISOR_WORKSPACE_FEATURES}
      workspaceContent={<div className="workbench-tool-target flex min-h-0 flex-1 flex-col" data-testid="workbench-tool-target" data-device={toolsDevice} data-workspace={toolsDetail?.workspace.id ?? ''} data-thread={toolsDetail?.thread.id ?? ''}>{toolsDetail ? <ThreadGraphWorkspacePanel detail={toolsDetail} status={toolsUseReference ? null : status} plugins={plugins} workspaceAdapter={workspaceAdapter} activeView="chat" features={SUPERVISOR_WORKSPACE_FEATURES} focusPathRequest={workspaceFocusPathRequest} onActiveFileChange={setToolsFile} /> : <p className="workbench-reference-status">{translate('workbench.loadingThreadDetail')}</p>}
        {toolsDetail && toolsCanControl ? <FileAskBar
          disabledReason={toolsUseReference ? null : promptDisabledReason}
          value={toolsDraft.prompt} attachmentCount={toolsDraft.attachments.length}
          onChange={prompt => setToolsDraft(draft => ({ ...draft, prompt }))}
          file={toolsFile}
          onSend={sendFromFiles} /> : null}</div>}
      workspaceFocusPathRequest={workspaceFocusPathRequest}
      activeView={activeView}
      liveOutput={liveOutput}
      timelineProps={timelineProps}
      timelineComponent={ThreadTimeline}
      useFloatingMobileComposer={false}
      floatingMobileComposerBottomOffset={floatingMobileComposerBottomOffset}
      composerHostRef={composerHostRef}
      shellContent={<></>}
      shellPanelRef={shellPanelRef}
      shellPanelComponent={ThreadShellPanel}
      shellEffectiveTheme={shellNav?.effectiveTheme ?? 'dark'}
      shellThemeMode={shellNav?.themeMode ?? 'system'}
      {...(shellNav?.setThemeMode
        ? { onShellThemeModeChange: shellNav.setThemeMode }
        : {})}
      onShellStateChange={setShellControlState}
      emptyContent={
        <div className="host-muted flex flex-1 items-center justify-center px-6 py-12 text-center">
          {translate("workbench.unableToResolveThisThread")}</div>
      }
      dialogs={<>{dialogs}{settingsDialogVisible && dialogDetail && <HarnessSettingsDialog
        key={`${dialogDevice}:${dialogDetail.thread.id}`} thread={dialogDetail.thread}
        models={dialogUseReference ? referenceController.models : modelOptions}
        busy={dialogUseReference ? referenceController.busy : settingsBusy}
        readOnly={dialogUseReference ? !referenceController.canControl : !relayThreadCanControl}
        {...(harnessSettingsTarget?.composerFocusOwner ? {composerFocusOwner:harnessSettingsTarget.composerFocusOwner} : {})}
        loadCapabilities={loadDialogCapabilities}
        {...harnessPanel(dialogDetail.thread.id, harnessSettingsTarget?.deviceId)}
        harnessReadOnly={dialogUseReference ? !referenceHarnessOwner : !relayThreadIsOwner}
        onChange={async input => { if (dialogUseReference) await referenceController.updateSettings(input); else await handleUpdateThreadSettings(input); }}
        onClose={() => setHarnessSettingsTarget(null)}
      />}</>}
      {...(chatComposerProps ? { composerProps: chatComposerProps } : {})}
      {...(shellComposerProps ? { shellComposerProps } : {})}
    />
    </>
  );
}
