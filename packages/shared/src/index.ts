import type { AgentBackendIdDto } from './agent-providers';

export {
  agentBackendIds,
  agentBackendMetadata,
  defaultAgentBackendId,
  isAgentBackendId,
  normalizeAgentBackendId,
} from './agent-providers';
export type {
  AgentBackendIdDto,
  AgentBackendMetadata,
} from './agent-providers';

export type ApiErrorCode =
  | 'fileAlreadyExists'
  | 'permissionDenied'
  | 'reauthentication_required'
  | 'rate_limited'
  | 'bad_request'
  | 'unauthorized'
  | 'not_found'
  | 'conflict'
  | 'confirmation_required'
  | 'provider_goal_error'
  | 'forbidden'
  | 'unauthorized'
  | 'invalid_route_token'
  | 'gateway_unavailable'
  | 'account_inactive'
  | 'quota_exceeded'
  | 'harness_unavailable'
  | 'goal_feature_disabled'
  | 'internal_error'
  | 'service_unavailable';

export interface ApiErrorShape {
  code: ApiErrorCode;
  message: string;
  details?: Record<string, unknown>;
}

const AUTO_THREAD_TITLE_MAX_CHARS = 15;

function normalizeAutoThreadTitleWhitespace(value: string) {
  return value.replace(/\s+/g, ' ').trim();
}

export function truncateAutoThreadTitle(value: string) {
  const normalized = normalizeAutoThreadTitleWhitespace(value);
  if (!normalized) {
    return '';
  }

  const characters = Array.from(normalized);
  if (characters.length <= AUTO_THREAD_TITLE_MAX_CHARS) {
    return normalized;
  }

  return `${characters.slice(0, AUTO_THREAD_TITLE_MAX_CHARS).join('')}...`;
}

export interface RuntimeConfigDto {
  appName: string;
  appVersion: string;
  mode: 'local' | 'server' | 'relay';
  host: string;
  port: number;
  workspaceRoot: string;
  environment: string;
  platform?: string;
  architecture?: string;
  nodeVersion?: string;
  capabilities?: {
    terminal: boolean;
    tmux: boolean;
    managedSignals: boolean;
    windowsTaskScheduler: boolean;
  };
}

export interface AuthSessionDto {
  authenticated: boolean;
  username: string | null;
  expiresAt: string | null;
  mode: 'local' | 'server' | 'relay';
  authRequired: boolean;
}

export interface AuthLoginResultDto {
  token: string | null;
  session: AuthSessionDto;
}

export interface RelayHealthDto {
  status: 'ok';
  supervisorConnected: boolean;
  supervisorConnectedAt: string | null;
  lastSupervisorHeartbeatAt: string | null;
  supervisorCount?: number;
}

export type RelayUserRoleDto = 'admin' | 'user';

export interface RelayUserDto {
  avatarUrl?: string | null;
  id: string;
  email: string;
  username: string;
  role: RelayUserRoleDto;
  enabled: boolean;
  createdAt: string;
}

export interface RelayDeviceDto {
  id: string;
  ownerUserId: string;
  name: string;
  token: string | null;
  tokenPreview: string;
  connected: boolean;
  connectedAt: string | null;
  lastHeartbeatAt: string | null;
  createdAt: string;
}

export interface RelayAdminUserDto extends RelayUserDto {
  lastSeenAt: string | null;
  deviceCount: number;
  conversationCount: number;
}

export interface RelayAdminWorkspaceDto {
  id: string;
  label: string;
  absPath?: string | null;
}

export interface RelayAdminThreadDto {
  id: string;
  title: string;
  workspaceId: string | null;
  workspaceLabel: string | null;
  status: string | null;
  updatedAt: string | null;
}

export interface RelayAdminDeviceDto extends RelayDeviceDto {
  ownerUsername: string;
  ownerEmail: string;
  ipAddress: string | null;
  workspaces: RelayAdminWorkspaceDto[];
  threads: RelayAdminThreadDto[];
}

export interface RelayRegistrationSettingsDto {
  enabled: boolean;
  registrationPasswordConfigured?: boolean;
  registrationPassword?: string | null;
  approvalRequired: boolean;
  googleAuthEnabled: boolean;
  githubAuthEnabled: boolean;
  emailVerificationEnabled: boolean;
  googleAuthAvailable: boolean;
  githubAuthAvailable: boolean;
  emailVerificationAvailable: boolean;
}

export interface RelayPendingRegistrationDto {
  id: string;
  email: string;
  username: string;
  createdAt: string;
  provider?: 'password' | 'google' | 'github';
}

export type RelayThreadAccessDto = 'read' | 'control';
export type RelayWorkspaceAccessDto = 'none' | 'read' | 'write';
export type RelayShareScopeDto = 'thread' | 'workspace' | 'device';
export type RelayWorkspaceScopeDto = 'all' | 'selected';

export interface CreateRelaySessionShareInput {
  targetIdentifier: string;
  deviceId: string;
  threadId: string;
  threadTitle?: string | null;
  workspaceId?: string | null;
  workspaceLabel?: string | null;
  label?: string | null;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  expiresAt?: string | null;
}

export interface UpdateRelaySessionShareInput {
  threadTitle?: string | null | undefined;
  workspaceId?: string | null | undefined;
  workspaceLabel?: string | null | undefined;
  label?: string | null | undefined;
  threadAccess?: RelayThreadAccessDto | undefined;
  workspaceAccess?: RelayWorkspaceAccessDto | undefined;
  expiresAt?: string | null | undefined;
}

export interface RelaySessionShareDto {
  /** Live supervisor connection; absent on older relays. */
  deviceConnected?: boolean;
  id: string;
  ownerUserId: string;
  ownerUsername: string;
  targetUsername: string;
  targetUserId: string;
  deviceId: string;
  deviceName: string;
  threadId: string;
  threadTitle: string | null;
  workspaceId: string | null;
  workspaceLabel: string | null;
  label: string | null;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  createdAt: string;
  revokedAt: string | null;
  expiresAt: string | null;
  lastAccessedAt: string | null;
  lastAccessedByUsername: string | null;
  accessEvents: RelaySessionShareAccessDto[];
}

export interface RelaySessionShareAccessDto {
  id: string;
  shareId: string;
  userId: string;
  username: string;
  kind: RelayAccessEventKindDto;
  accessedAt: string;
}

export type RelayAccessEventKindDto =
  | 'access'
  | 'open_device'
  | 'open_thread'
  | 'create_thread'
  | 'send_prompt'
  | 'read_workspace_file'
  | 'write_workspace_file';

export interface RelayAccessGrantEventDto {
  id: string;
  grantId: string;
  userId: string;
  username: string;
  kind: RelayAccessEventKindDto;
  accessedAt: string;
}

export interface CreateRelayAccessGrantInput {
  targetIdentifier: string;
  deviceId: string;
  scope: RelayShareScopeDto;
  threadId?: string | null;
  workspaceId?: string | null;
  workspaceScope?: RelayWorkspaceScopeDto;
  workspaceIds?: string[];
  label?: string | null;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  canCreateThreads?: boolean;
  expiresAt?: string | null;
}

export interface UpdateRelayAccessGrantInput {
  workspaceId?: string | null | undefined;
  workspaceScope?: RelayWorkspaceScopeDto | undefined;
  workspaceIds?: string[] | undefined;
  label?: string | null | undefined;
  threadAccess?: RelayThreadAccessDto | undefined;
  workspaceAccess?: RelayWorkspaceAccessDto | undefined;
  canCreateThreads?: boolean | undefined;
  expiresAt?: string | null | undefined;
}

export interface RelayAccessGrantDto {
  deviceConnected?: boolean;
  id: string;
  ownerUserId: string;
  ownerUsername: string;
  targetUserId: string;
  targetUsername: string;
  deviceId: string;
  deviceName: string;
  scope: RelayShareScopeDto;
  threadId: string | null;
  threadTitle: string | null;
  workspaceId: string | null;
  workspaceLabel: string | null;
  workspaceScope: RelayWorkspaceScopeDto;
  workspaceIds: string[];
  label: string | null;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  canCreateThreads: boolean;
  createdAt: string;
  revokedAt: string | null;
  expiresAt: string | null;
  lastAccessedAt: string | null;
  lastAccessedByUsername: string | null;
  accessEvents: RelayAccessGrantEventDto[];
}

export interface RelayEffectiveAccessDto {
  kind: 'owner' | 'shared';
  grantId?: string | null;
  shareId: string | null;
  scope?: 'owner' | RelayShareScopeDto;
  threadAccess: RelayThreadAccessDto;
  workspaceAccess: RelayWorkspaceAccessDto;
  workspaceId: string | null;
  workspaceScope?: RelayWorkspaceScopeDto | null;
  canCreateThreads?: boolean;
}

export interface RelaySessionDto {
  authenticated: boolean;
  user: RelayUserDto | null;
  registrationEnabled: boolean;
  registrationSettings?: RelayRegistrationSettingsDto;
}

export interface RelayLoginResultDto {
  token?: string;
  challengeRequired?: boolean;
  authenticator?: boolean;
  passkey?: boolean;
  session: RelaySessionDto;
}

export interface RelayRegisterResultDto {
  token?: string;
  session?: RelaySessionDto;
  pendingApproval?: boolean;
  request?: RelayPendingRegistrationDto;
}

export interface RelayCreateDeviceResultDto {
  device: RelayDeviceDto;
  token: string;
}

export interface RelayPortalSummaryDto {
  user: RelayUserDto;
  devices: RelayDeviceDto[];
  sharedWithMe: RelaySessionShareDto[];
  sharedByMe: RelaySessionShareDto[];
  sharedDevicesWithMe?: RelayAccessGrantDto[];
  sharedThreadsWithMe?: RelayAccessGrantDto[];
  grantsByMe?: RelayAccessGrantDto[];
}

export interface RelayAdminSummaryDto {
  users: RelayAdminUserDto[];
  devices: RelayAdminDeviceDto[];
  shares: RelaySessionShareDto[];
  grants?: RelayAccessGrantDto[];
  pendingRegistrations: RelayPendingRegistrationDto[];
  settings: RelayRegistrationSettingsDto;
  conversationWindowDays: number;
  registrationEnabled: boolean;
}

export type RelaySupervisorEnvelope =
  | {
      type: 'relay.connected';
      timestamp: string;
      deviceId?: string;
    }
  | {
      type: 'relay.heartbeat';
      timestamp: string;
      deviceId?: string;
    }
  | {
      type: 'relay.request';
      timestamp: string;
      requestId: string;
      deviceId?: string;
      payload: RelayHttpRequestPayload;
    }
  | {
      type: 'relay.response';
      timestamp: string;
      requestId: string;
      deviceId?: string;
      payload: RelayHttpResponsePayload;
    }
  | {
      type: 'relay.client.connected';
      timestamp: string;
      clientId: string;
    }
  | {
      type: 'relay.client.disconnected';
      timestamp: string;
      clientId: string;
    }
  | {
      type: 'relay.client.message';
      timestamp: string;
      clientId: string;
      payload: SupervisorSocketClientEnvelope;
    }
  | {
      type: 'relay.server.message';
      timestamp: string;
      clientId: string;
      payload: SupervisorSocketServerEnvelope;
    };

export interface RelayHttpRequestPayload {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: string | null;
  bodyEncoding?: 'utf8' | 'base64';
}

export interface RelayHttpResponsePayload {
  statusCode: number;
  headers: Record<string, string>;
  body: string;
  bodyEncoding?: 'utf8' | 'base64';
}

export interface AgentRuntimeStatusDto {
  state: 'starting' | 'ready' | 'degraded' | 'stopped' | 'failed';
  transport: 'stdio' | 'sdk' | 'none';
  lastStartedAt: string | null;
  lastError: string | null;
  restartCount: number;
  operationalMetrics?: {
    sessionStartFailures: number;
    resumeFailures: number;
    capabilityProbeFailures: number;
  };
}

export interface AgentSubscriptionUsageWindowDto {
  id: string;
  durationMinutes: number | null;
  label: string;
  usedPercent: number;
  resetsAt: string | null;
}

export interface AgentSubscriptionUsageDto {
  provider: 'codex' | 'claude' | 'grok';
  authKind: 'subscription' | 'apiKey' | 'unknown';
  observedAt: string;
  stale: boolean;
  windows: AgentSubscriptionUsageWindowDto[];
}

export interface AgentProviderCapabilitiesDto {
  sessions: {
    list: boolean;
    read: boolean;
    resume: boolean;
    importLocal: boolean;
    load?: boolean;
    close?: boolean;
    delete?: boolean;
  };
  turns: {
    start: boolean;
    streamInput: boolean;
    steer: boolean;
    interrupt: boolean;
    compact: boolean;
  };
  branching: {
    fork: boolean;
    forkAt?: boolean;
    hardRollback: boolean;
    resumeAt: boolean;
    rewindFiles: boolean;
  };
  controls: {
    planMode: boolean;
    permissionRequests: boolean;
    sandboxMode: boolean;
    performanceMode: boolean;
    goals: boolean;
  };
  management: {
    models: boolean;
    mcpStatus: boolean;
    skills: boolean;
    hooks: boolean;
    hookTrust: boolean;
    hostConfigFiles: boolean;
    providerSettings: boolean;
  };
  usage: {
    contextWindow: boolean;
    tokenUsage: boolean;
    costUsd: boolean;
  };
}

export interface AgentBackendDto {
  provider: AgentBackendIdDto;
  displayName: string;
  description: string;
  enabled: boolean;
  isDefault: boolean;
  status: AgentRuntimeStatusDto;
  capabilities: AgentProviderCapabilitiesDto;
  managementSchema: AgentBackendManagementSchemaDto;
  installation: AgentBackendInstallationDto;
}

export interface AgentCapabilitySnapshotDto {
  provider: AgentBackendIdDto;
  agentId: string;
  availability: AcpAgentOptionMetadataDto['availability'];
  negotiated: unknown | null;
  effectiveCapabilities: AgentProviderCapabilitiesDto | null;
  toolboxItems: AgentBackendToolboxItemSchemaDto[];
}

export interface AgentBackendInstallationDto {
  packageName: string | null;
  installed: boolean;
  installedVersion: string | null;
  latestVersion: string | null;
  installCommand: string | null;
  updateCommand: string | null;
  busy: boolean;
  lastError: string | null;
}

export interface AgentBackendConfigFileSchemaDto {
  name: string;
  label: string;
  description: string;
  roles?: Array<'runtime' | 'auth' | 'mcp' | 'hooks' | 'providerSettings'>;
}

export type AgentBackendToolboxActionDto =
  | 'fast'
  | 'compact'
  | 'goal'
  | 'fork'
  | 'skills'
  | 'mcp'
  | 'hooks'
  | 'prompt'
  | 'harness'
  | 'unsupported';

export interface AgentBackendToolboxItemSchemaDto {
  action: AgentBackendToolboxActionDto;
  command: string;
  label: string;
  description?: string | null;
  panel?: 'fork' | 'skills' | 'mcp' | 'hooks' | null;
}

export interface AgentBackendHookCommandTemplateDto {
  eventName: AgentHookEventNameDto;
  command: string;
}

export interface AgentBackendManagementSchemaDto {
  hostConfigFiles: AgentBackendConfigFileSchemaDto[];
  toolboxItems: AgentBackendToolboxItemSchemaDto[];
  hookCommandTemplates: AgentBackendHookCommandTemplateDto[];
  providerConfigFormat: 'toml' | 'json' | 'none';
  mcpConfigFormat: 'codex-toml' | 'claude-json' | 'none';
  configArchives: boolean;
  buildRestart: boolean;
}

export interface ModelOptionDto {
  id: string;
  model: string;
  displayName: string;
  description: string;
  isDefault: boolean;
  hidden: boolean;
  supportsPerformanceMode?: boolean;
  supportedReasoningEfforts: ReasoningEffortOptionDto[];
  defaultReasoningEffort: ReasoningEffortDto | null;
  selectionKind?: 'model' | 'agent';
  acpAgent?: AcpAgentOptionMetadataDto | null;
}

export type AcpAgentAvailabilityDto =
  | 'ready'
  | 'base_missing'
  | 'adapter_missing'
  | 'server_unavailable';

export interface AcpAgentOptionMetadataDto {
  transport: 'native' | 'adapter' | 'custom';
  availability: AcpAgentAvailabilityDto;
  baseCommand: string;
  baseProbeCommand: string;
  serverCommand: string;
  serverProbeCommand: string;
  baseVersion: string | null;
  serverVersion: string | null;
  installCommand: string | null;
  busy: boolean;
  statusMessage: string;
}

export interface VersionDto {
  name: string;
  version: string;
}

export interface HealthDto {
  status: 'ok';
  timestamp: string;
  activeTurnCount: number;
}

export type ProviderHostFileNameDto = string;

export interface ProviderHostFileDto {
  name: ProviderHostFileNameDto;
  path: string;
  exists: boolean;
  content: string;
}

export interface UpdateProviderHostFileInput {
  content: string;
}

export interface ProviderHostConfigArchiveFileDto {
  name: ProviderHostFileNameDto;
  exists: boolean;
}

export interface ProviderHostConfigArchiveDto {
  id: string;
  label: string;
  createdAt: string;
  updatedAt: string;
  files: Record<string, ProviderHostConfigArchiveFileDto>;
}

export interface CreateProviderHostConfigArchiveInput {
  label?: string;
}

export interface RenameProviderHostConfigArchiveInput {
  label: string;
}

export interface ApplyProviderHostConfigArchiveResultDto {
  archive: ProviderHostConfigArchiveDto;
  status: AgentRuntimeStatusDto;
}

export interface WorkspaceDto {
  id: string;
  hostId: string;
  label: string;
  absPath: string;
  isFavorite: boolean;
  createdAt: string;
  lastOpenedAt: string | null;
}

export interface CreateWorkspaceFromPathInput {
  absPath: string;
  label?: string;
}

export interface CreateWorkspaceFromGitInput {
  gitUrl: string;
  label?: string;
}

export type CreateWorkspaceInput =
  | CreateWorkspaceFromPathInput
  | CreateWorkspaceFromGitInput;

export interface WorkspaceSettingsDto {
  workspaceRoot: string;
  devHome: string;
  defaultBackend: AgentBackendIdDto;
}

export interface UpdateWorkspaceSettingsInput {
  devHome: string;
  defaultBackend?: AgentBackendIdDto;
}

export interface UpdateWorkspaceInput {
  label: string;
}

export type ThreadSourceDto =
  | 'supervisor'
  | 'local_codex_import'
  | 'local_provider_import';

export interface UpdateWorkspaceFavoriteInput {
  isFavorite: boolean;
}

export interface WorkspaceTreeNodeDto {
  name: string;
  absPath: string;
  kind: 'file' | 'directory';
  hasChildren: boolean;
  isHidden: boolean;
}

export interface WorkspaceTreeDto {
  rootPath: string;
  currentPath: string;
  nodes: WorkspaceTreeNodeDto[];
}

export interface WorkspaceFileDto {
  path: string;
  absPath: string;
  kind: 'file' | 'directory';
  size: number;
  updatedAt: string;
}

export interface WriteWorkspaceFileInput {
  path: string;
  content: string;
}

export interface MoveWorkspaceFileInput {
  fromPath: string;
  toPath: string;
  overwrite?: boolean;
}

export interface DeleteWorkspaceFileInput {
  path: string;
  recursive?: boolean;
}

export interface ThreadWorkspaceTreeNodeDto {
  name: string;
  path: string;
  kind: 'file' | 'directory';
  size?: number;
  hasChildren?: boolean;
  childrenLoaded?: boolean;
  truncated?: boolean;
  children?: ThreadWorkspaceTreeNodeDto[];
}

export interface ThreadWorkspaceFilePreviewDto {
  path: string;
  name: string;
  content: string;
  language: string;
  size: number;
  truncated: boolean;
  nextOffset: number;
}

export type ThreadWorkspaceUploadResultDto =
  | {
      kind: 'file';
      file: {
        path: string;
        name: string;
        size: number;
      };
    }
  | {
      kind: 'archive';
      archiveName: string;
      extractedCount: number;
      paths: string[];
    };

export type ApprovalMode = 'yolo' | 'guarded';
export type ReasoningEffortDto =
  | 'none'
  | 'minimal'
  | 'low'
  | 'medium'
  | 'high'
  | 'xhigh'
  | 'max'
  | 'ultra'
  | (string & {}); // ACP values are opaque, including the provider-default empty value.
export type CollaborationModeDto = 'default' | 'plan';
export type SandboxModeDto =
  | 'read-only'
  | 'workspace-write'
  | 'danger-full-access';

export interface ReasoningEffortOptionDto {
  reasoningEffort: ReasoningEffortDto;
  description: string;
}

export type ThreadStatusDto =
  | 'recovering'
  | 'idle'
  | 'running'
  | 'interrupted'
  | 'failed'
  | 'not_loaded'
  | 'system_error';

export interface ThreadContextUsageDto {
  availability: 'available' | 'unavailable';
  remainingPercent: number | null;
  tokensInContextWindow: number | null;
  modelContextWindow: number | null;
  updatedAt: string | null;
}

export interface ThreadDto {
  id: string;
  workspaceId: string;
  provider: AgentBackendIdDto;
  agentId?: string | null;
  providerSessionId: string | null;
  source: ThreadSourceDto;
  title: string;
  model: string | null;
  reasoningEffort: ReasoningEffortDto | null;
  fastMode?: boolean;
  collaborationMode: CollaborationModeDto;
  approvalMode: ApprovalMode;
  sandboxMode?: SandboxModeDto | null;
  status: ThreadStatusDto;
  summaryText: string | null;
  lastError: string | null;
  activeTurnId: string | null;
  isLoaded: boolean;
  isPinned: boolean;
  createdAt: string;
  updatedAt: string;
  lastTurnStartedAt: string | null;
  lastTurnCompletedAt: string | null;
  contextUsage?: ThreadContextUsageDto;
  /** Immediate creator. Absent when a person started this thread. */
  parentThreadId?: string | null;
  /** Top-level ancestor; every thread in one delegation chain shares it. */
  rootThreadId?: string | null;
  /** 0 for a root, parent + 1 otherwise. */
  lineageDepth?: number;
  /** Agent threads beneath this root. Only present on roots in list responses. */
  descendantCount?: number | null;
  /** Address unique among the open threads of one lineage, e.g. `reviewer`. */
  agentName?: string | null;
  /** Role template the thread started from. */
  agentRole?: string | null;
  /** Isolated git worktree the thread runs in instead of the workspace root. */
  worktreePath?: string | null;
  /** Set once the delegate was closed; it then holds no slot and no name. */
  closedAt?: string | null;
}

export interface ThreadPeerWorkDto {
  taskTurnId: string;
  messages: Array<{
    id: string;
    fromThreadId?: string | null;
    fromDeviceId?: string | null;
    name?: string | null;
    kind: 'question' | 'result';
    subject?: string | null;
    text?: string | null;
    createdAt?: string | null;
  }>;
}

export interface ThreadHistoryItemDto {
  peerWork?: ThreadPeerWorkDto | null;
  id: string;
  createdAt?: string | null;
  kind:
    | 'userMessage'
    | 'agentMessage'
    | 'artifact'
    | 'image'
    | 'plan'
    | 'contextCompaction'
    | 'reasoning'
    | 'commandExecution'
    | 'webSearch'
    | 'fileRead'
    | 'fileChange'
    | 'hook'
    | 'agentToolCall'
    | 'skillToolCall'
    | 'toolCall'
    | 'other';
  text: string;
  previewText?: string;
  detailText?: string | null;
  hasDeferredDetail?: boolean | null;
  sequence?: number | null;
  transcriptOrder?: number | null;
  sourceTurnId?: string | null;
  origin?: string | null;
  taskStatus?: string | null;
  taskId?: string | null;
  waitingStartedAt?: string | null;
  awakenedAt?: string | null;
  backgroundTaskCount?: number | null;
  responsePhase?: 'commentary' | 'final' | null;
  status?: string | null;
  assetPath?: string | null;
  changedFiles?: number | null;
  addedLines?: number | null;
  removedLines?: number | null;
  hookEventName?: string | null;
  hookEventLabel?: string | null;
  hookHandlerType?: string | null;
  hookScope?: string | null;
  hookSource?: string | null;
  hookSourcePath?: string | null;
  hookStatusMessage?: string | null;
  hookOutputEntries?: Array<{
    kind: string;
    text: string;
  }> | null;
  artifact?: ThreadArtifactDto | null;
}

export interface ThreadHistoryItemDetailDto {
  id: string;
  kind: ThreadHistoryItemDto['kind'];
  title: string;
  text: string;
}

export interface ThreadArtifactDto {
  id: string;
  pluginId: string;
  type: string;
  title: string;
  summaryText?: string | null;
  payload: unknown;
  assets?: Array<{
    id: string;
    mediaType: string;
    url: string;
    name?: string | null;
  }> | null;
  sourceTurnId?: string | null;
  sourceItemId?: string | null;
  createdAt: string;
}

export interface PluginArtifactTypeDto {
  type: string;
  title: string;
  fileExtensions?: string[];
}

export interface PluginThreadPanelDto {
  id: string;
  label: string;
  kind?: 'artifact' | 'terminal' | string;
  artifactTypes: string[];
}

export interface PluginModelHintDto {
  id: string;
  text: string;
}

export interface PluginMcpServerDto {
  id: string;
  name: string;
  command: string;
  args?: string[];
  env?: Record<string, string>;
}

export interface PluginCapabilitiesDto {
  artifactTypes: PluginArtifactTypeDto[];
  timelineRenderers: string[];
  threadPanels: PluginThreadPanelDto[];
  modelHints?: PluginModelHintDto[];
  mcpServers?: PluginMcpServerDto[];
  frontend?: {
    entry?: string;
    style?: string;
  };
  backend?: {
    entry?: string;
  };
}

export interface PluginManifestDto {
  id: string;
  name: string;
  version: string;
  description: string;
  remoteCodex: string;
  capabilities: PluginCapabilitiesDto;
}

export interface PluginDto extends PluginManifestDto {
  enabled: boolean;
  source?: 'builtin' | 'imported' | null;
  available?: boolean;
  unavailableReasonCode?: 'unsupported_platform' | null;
  unavailableReason?: string | null;
}

export interface UpdatePluginInput {
  enabled: boolean;
}

export interface ImportPluginInput {
  manifest?: unknown;
  manifestJson?: string;
  manifestUrl?: string;
  enabled?: boolean;
}

export interface ThreadTurnTokenBreakdownDto {
  totalTokens: number;
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens?: number;
  /** One-hour subset of cacheWriteInputTokens. */
  cacheWriteOneHourInputTokens?: number;
  outputTokens: number;
  reasoningOutputTokens: number;
}

export interface ThreadTurnTokenUsageDto {
  total: ThreadTurnTokenBreakdownDto;
  last: ThreadTurnTokenBreakdownDto;
  modelContextWindow: number | null;
  generationSpeed?: {
    outputTokens: number;
    llmTimeMs: number;
    averageTokensPerSecond: number | null;
    recentTokensPerSecond: number | null;
    windowSeconds: number;
    active: boolean;
    state: 'llm' | 'tool';
    measurement: 'usageIntervals';
    outputTimeMs?: number;
    averageOutputTokensPerSecond?: number | null;
    latestOutputTokensPerSecond?: number | null;
    latestOutputTimeMs?: number;
    latestOutputMeasuredAt?: string | null;
    updatedAt: string;
  } | null;
}

export type ThreadTurnPricingTierDto = 'standard' | 'fast';

export interface ThreadTurnPriceEstimateDto {
  pricingModelKey: string;
  pricingTierKey: ThreadTurnPricingTierDto;
  currency: 'USD';
  inputUsd: number;
  cachedInputUsd: number;
  cacheWriteInputUsd?: number;
  outputUsd: number;
  totalUsd: number;
}

export interface ThreadTurnDto {
  id: string;
  startedAt: string | null;
  completedAt?: string | null;
  status: 'completed' | 'interrupted' | 'failed' | 'inProgress' | 'recovering';
  error: string | null;
  model?: string | null;
  reasoningEffort?: ReasoningEffortDto | null;
  reasoningEffortAvailable?: boolean | null;
  tokenUsage?: ThreadTurnTokenUsageDto | null;
  priceEstimate?: ThreadTurnPriceEstimateDto | null;
  hasDeferredItems?: boolean | null;
  deferredItemCount?: number | null;
  items: ThreadHistoryItemDto[];
}

export interface ThreadActionQuestionOptionDto {
  label: string;
  description: string;
}

export interface ThreadActionQuestionDto {
  id: string;
  header: string;
  question: string;
  multiSelect?: boolean;
  isOther: boolean;
  isSecret: boolean;
  options: ThreadActionQuestionOptionDto[] | null;
}

export interface ThreadActionRequestDto {
  id: string;
  kind: 'requestUserInput' | 'permissionRequest' | 'planDecision';
  title: string;
  description: string | null;
  turnId: string | null;
  itemId: string | null;
  createdAt: string;
  questions: ThreadActionQuestionDto[];
}

export interface ThreadAnsweredRequestNoteDto {
  id: string;
  turnId: string | null;
  title: string;
  summaryLines: string[];
  createdAt: string;
}

export interface ThreadActivityNoteDto {
  id: string;
  kind: 'fastMode' | 'goal' | 'forkCreated' | 'forkSource';
  createdAt: string;
  text?: string;
  anchorTurnId?: string | null;
  linkedThreadId?: string;
  linkedThreadTitle?: string | null;
  turnIndex?: number | null;
}

export type AgentSkillScopeDto = 'user' | 'repo' | 'system' | 'admin';

export interface AgentSkillInterfaceDto {
  displayName?: string;
  shortDescription?: string;
  brandColor?: string;
  defaultPrompt?: string;
}

export interface AgentSkillDto {
  name: string;
  description: string;
  shortDescription?: string;
  interface?: AgentSkillInterfaceDto;
  path: string;
  scope: AgentSkillScopeDto;
  enabled: boolean;
}

export interface AgentSkillErrorDto {
  path: string;
  message: string;
}

export interface ThreadSkillsDto {
  cwd: string;
  skills: AgentSkillDto[];
  errors: AgentSkillErrorDto[];
}

export type AgentMcpAuthStatusDto =
  | 'unsupported'
  | 'notLoggedIn'
  | 'bearerToken'
  | 'oAuth';

export interface AgentMcpToolDto {
  name: string;
  title: string | null;
  description: string | null;
}

export interface AgentMcpServerDto {
  name: string;
  authStatus: AgentMcpAuthStatusDto;
  tools: AgentMcpToolDto[];
  resourceCount: number;
  resourceTemplateCount: number;
}

export interface ThreadMcpServersDto {
  servers: AgentMcpServerDto[];
}

export type AgentHookEventNameDto =
  | 'preToolUse'
  | 'permissionRequest'
  | 'postToolUse'
  | 'preCompact'
  | 'postCompact'
  | 'sessionStart'
  | 'userPromptSubmit'
  | 'stop';

export type AgentHookHandlerTypeDto = 'command' | 'prompt' | 'agent';
export type AgentHookSourceDto =
  | 'system'
  | 'user'
  | 'project'
  | 'mdm'
  | 'sessionFlags'
  | 'plugin'
  | 'cloudRequirements'
  | 'legacyManagedConfigFile'
  | 'legacyManagedConfigMdm'
  | 'unknown';
export type AgentHookTrustStatusDto =
  | 'managed'
  | 'untrusted'
  | 'trusted'
  | 'modified';

export interface AgentHookDto {
  key: string;
  eventName: AgentHookEventNameDto;
  handlerType: AgentHookHandlerTypeDto;
  matcher: string | null;
  command: string | null;
  timeoutSec: number;
  statusMessage: string | null;
  sourcePath: string;
  source: AgentHookSourceDto;
  pluginId: string | null;
  displayOrder: number;
  enabled: boolean;
  isManaged: boolean;
  currentHash: string;
  trustStatus: AgentHookTrustStatusDto;
}

export interface AgentHookErrorDto {
  path: string;
  message: string;
}

export interface ThreadHooksDto {
  cwd: string;
  hooks: AgentHookDto[];
  warnings: string[];
  errors: AgentHookErrorDto[];
  globalHooksPath: string;
  projectHooksPath: string;
}

export interface CreateThreadHookInput {
  scope: 'global' | 'project';
  eventName: AgentHookEventNameDto;
  matcher?: string | null;
  command: string;
  timeoutSec?: number | null;
  statusMessage?: string | null;
}

export interface ThreadHookTargetInput {
  scope: 'global' | 'project';
  eventName: AgentHookEventNameDto;
  matcher?: string | null;
  command: string;
  timeoutSec?: number | null;
  statusMessage?: string | null;
}

export interface UpdateThreadHookInput extends CreateThreadHookInput {
  target: ThreadHookTargetInput;
}

export interface TrustThreadHookInput {
  key: string;
  currentHash: string;
}

export interface UntrustThreadHookInput {
  key: string;
}

export type ThreadGoalStatusDto =
  | 'active'
  | 'paused'
  | 'budgetLimited'
  | 'complete'
  | 'terminated';

export interface ThreadGoalDto {
  threadId: string;
  localGoalId?: string | null;
  objective: string;
  status: ThreadGoalStatusDto;
  tokenBudget: number | null;
  tokensUsed: number;
  timeUsedSeconds: number;
  createdAt: string;
  updatedAt: string;
  completedAt?: string | null;
}

export interface UpdateThreadGoalInput {
  objective?: string | null;
  status?: ThreadGoalStatusDto | null;
  tokenBudget?: number | null;
}

export interface ThreadLivePlanDto {
  turnId: string;
  explanation: string | null;
  plan: Array<{ step: string; status: string }>;
  updatedAt: string;
}

export interface ThreadPendingSteerDto {
  id: string;
  clientRequestId: string | null;
  turnId: string;
  prompt: string;
  delivery: 'steer' | 'continuation';
  createdAt: string;
}

export interface ThreadSubagentDto {
  id: string;
  name: string | null;
  status: string;
  startedAt: string | null;
  completedAt: string | null;
  parentToolCallId: string | null;
  isBackground?: boolean;
}

export interface NativeSubagentDto extends ThreadSubagentDto {
  provider: 'codex' | 'claude' | string;
  nativeSessionId: string | null;
  model: string | null;
  prompt: string | null;
  updatedAt: string | null;
  latestActivity: string | null;
  tokenUsage: ThreadTurnTokenUsageDto | null;
  priceEstimate: ThreadTurnPriceEstimateDto | null;
  activityCount: number;
  detailsAvailable: boolean;
}

export interface NativeSubagentDetailDto {
  historyMode?: 'lazy-v1';
  agent: NativeSubagentDto;
  items: ThreadHistoryItemDto[];
  hasEarlierItems: boolean;
}

export interface ThreadLiveItemsDto {
  turnId: string;
  items: ThreadHistoryItemDto[];
  updatedAt: string;
}

export interface ThreadDetailDto {
  thread: ThreadDto;
  workspace: WorkspaceDto;
  workspacePathStatus: 'present' | 'missing';
  turns: ThreadTurnDto[];
  totalTurnCount?: number;
  pendingRequests: ThreadActionRequestDto[];
  pendingSteers: ThreadPendingSteerDto[];
  activeSubagents?: ThreadSubagentDto[];
  answeredRequestNotes?: ThreadAnsweredRequestNoteDto[];
  activityNotes?: ThreadActivityNoteDto[];
  goal?: ThreadGoalDto | null;
  goalHistory?: ThreadGoalDto[];
  livePlan?: ThreadLivePlanDto | null;
  liveItems?: ThreadLiveItemsDto | null;
}

export function prependEarlierThreadTurns(
  existing: ThreadDetailDto['turns'],
  earlier: ThreadDetailDto['turns'],
) {
  const existingIds = new Set(existing.map((turn) => turn.id));
  return [...earlier.filter((turn) => !existingIds.has(turn.id)), ...existing];
}

export function mergeLatestThreadTurns(
  existing: ThreadDetailDto['turns'],
  latest: ThreadDetailDto['turns'],
  activeTurnId: string | null = null,
) {
  const seenLatestIds = new Set<string>();
  const uniqueLatest = latest.reduceRight<ThreadDetailDto['turns']>(
    (turns, turn) => {
      if (!seenLatestIds.has(turn.id)) {
        seenLatestIds.add(turn.id);
        turns.unshift(turn);
      }
      return turns;
    },
    [],
  );
  const latestById = new Map(uniqueLatest.map((turn) => [turn.id, turn]));
  const latestReplacements = uniqueLatest.filter((turn) =>
    turn.items.some((item) => item.kind === 'userMessage'),
  );
  const replacedExistingIds = new Set<string>();
  const seenExistingIds = new Set<string>();
  const uniqueExisting = existing.reduceRight<ThreadDetailDto['turns']>(
    (turns, turn) => {
      if (!seenExistingIds.has(turn.id)) {
        seenExistingIds.add(turn.id);
        turns.unshift(turn);
      }
      return turns;
    },
    [],
  );

  for (const turn of uniqueExisting) {
    if (latestById.has(turn.id)) {
      replacedExistingIds.add(turn.id);
      continue;
    }
    if (turn.status !== 'inProgress') {
      continue;
    }
    const existingPrompt = turn.items
      .find((item) => item.kind === 'userMessage')
      ?.text.trim();
    const existingStartedAt = turn.startedAt
      ? Date.parse(turn.startedAt)
      : Number.NaN;
    if (!existingPrompt || !Number.isFinite(existingStartedAt)) {
      continue;
    }
    const materializedReplacement = latestReplacements.find((replacement) => {
      const replacementPrompt = replacement.items
        .find((item) => item.kind === 'userMessage')
        ?.text.trim();
      const replacementStartedAt = replacement.startedAt
        ? Date.parse(replacement.startedAt)
        : Number.NaN;
      return (
        replacementPrompt === existingPrompt &&
        Number.isFinite(replacementStartedAt) &&
        Math.abs(replacementStartedAt - existingStartedAt) <= 15_000
      );
    });
    if (materializedReplacement) {
      replacedExistingIds.add(turn.id);
    }
  }

  const olderTurns = uniqueExisting.filter(
    (turn) => !latestById.has(turn.id) && !replacedExistingIds.has(turn.id),
  );
  const ordered = [...olderTurns, ...uniqueLatest];
  if (!activeTurnId) {
    return ordered;
  }

  const activeTurn = ordered.find((turn) => turn.id === activeTurnId);
  return activeTurn
    ? [...ordered.filter((turn) => turn.id !== activeTurnId), activeTurn]
    : ordered;
}

export interface ThreadExportTurnOptionDto {
  turnId: string;
  turnNumber: number;
  startedAt: string | null;
  status: ThreadTurnDto['status'];
  userPromptPreview: string;
}

export interface ThreadExportTurnOptionsDto {
  turns: ThreadExportTurnOptionDto[];
  totalTurnCount: number;
}

export type ThreadExportPdfModeDto = 'latest' | 'selected';
export type ThreadExportPdfProfileDto = 'review' | 'technical';
export type ThreadExportFormatDto = 'html';

export interface ExportThreadTranscriptInput {
  format?: ThreadExportFormatDto;
  mode: ThreadExportPdfModeDto;
  limit?: number;
  turnIds?: string[];
  profile?: ThreadExportPdfProfileDto;
  options?: {
    includeTokenAndPrice?: boolean;
    includeCommandOutput?: boolean;
    includeAbsolutePaths?: boolean;
  };
}

export interface ThreadForkTurnOptionDto {
  turnId: string;
  turnIndex: number;
  startedAt: string | null;
  status: ThreadTurnDto['status'];
}

export interface ForkThreadInput {
  mode: 'latest' | 'turn';
  turnId?: string;
}

export interface ThreadForkResultDto {
  thread: ThreadDetailDto;
  sourceThreadId: string;
  sourceTurnId: string | null;
  sourceTurnIndex: number | null;
}

export type ShellStatusDto =
  | 'not_created'
  | 'creating'
  | 'running'
  | 'attached'
  | 'detached'
  | 'exited'
  | 'not_found'
  | 'workspace_missing';

export interface ShellSessionDto {
  id: string;
  threadId: string;
  workspaceId: string;
  label: string | null;
  tmuxSessionName: string;
  backend: 'pty' | 'tmux' | string;
  cwd: string;
  status: Exclude<ShellStatusDto, 'not_created' | 'workspace_missing'>;
  attachedViewerId: string | null;
  createdAt: string;
  updatedAt: string;
  lastActivityAt: string | null;
}

export interface ThreadShellStateDto {
  threadId: string;
  workspaceId: string;
  workspacePathStatus: 'present' | 'missing';
  state: ShellStatusDto;
  shell: ShellSessionDto | null;
  shells: ShellSessionDto[];
  activeShellId: string | null;
}

export interface ShellCreateInput {
  cols?: number;
  rows?: number;
  label?: string;
}

export interface UpdateShellInput {
  label?: string | null;
}

export interface ShellAttachInput {
  cols: number;
  rows: number;
}

export interface ShellDetachInput {
  viewerId: string;
}

export interface ShellInputInput {
  viewerId: string;
  data: string;
}

export interface ShellResizeInput {
  viewerId: string;
  cols: number;
  rows: number;
}

export interface CreateThreadInput {
  workspaceId: string;
  title?: string;
  provider?: AgentBackendIdDto;
  agentId?: string;
  model: string;
  reasoningEffort?: ReasoningEffortDto | null;
  approvalMode: ApprovalMode;
}

export interface UpdateThreadSettingsInput {
  model?: string;
  reasoningEffort?: ReasoningEffortDto | null;
  fastMode?: boolean;
  collaborationMode?: CollaborationModeDto;
  sandboxMode?: SandboxModeDto | null;
}

export interface UpdateThreadInput {
  title: string;
}

export interface ImportThreadInput {
  sessionId: string;
  provider?: AgentBackendIdDto;
  agentId?: string | null;
}

export interface ImportThreadCandidateDto {
  provider: AgentBackendIdDto;
  agentId: string | null;
  sessionId: string;
  cwd: string;
  title: string;
  preview: string | null;
  createdAt: string | null;
  updatedAt: string | null;
  historyStatus: 'unknown';
}

export interface SendThreadPromptInput {
  prompt: string;
  clientRequestId?: string;
  model?: string;
  reasoningEffort?: ReasoningEffortDto | null;
  collaborationMode?: CollaborationModeDto;
  sandboxMode?: SandboxModeDto | null;
}

export type PromptAttachmentKindDto = 'photo' | 'file';

export interface PromptAttachmentManifestEntryDto {
  clientId: string;
  kind: PromptAttachmentKindDto;
  originalName: string;
  placeholder: string;
}

export interface InterruptTurnInput {
  turnId?: string;
}

export interface ResumeThreadInput {
  model?: string;
  sandboxMode?: SandboxModeDto | null;
}

export interface ThreadActionRequestAnswerDto {
  answers: string[];
}

export interface RespondThreadActionRequestInput {
  answers: Record<string, ThreadActionRequestAnswerDto>;
}

export interface ThreadEventPayloadMap {
  'thread.updated': {
    pendingSteers?: ThreadDetailDto['pendingSteers'];
    status?: ThreadStatusDto | string | null;
    title?: string | null;
    reason?: string;
    turnId?: string;
    model?: string | null;
    reasoningEffort?: ReasoningEffortDto | string | null;
    fastMode?: boolean;
    collaborationMode?: CollaborationModeDto | string | null;
    sandboxMode?: SandboxModeDto | string | null;
  };
  'thread.context.updated': {
    contextUsage: ThreadContextUsageDto;
  };
  'thread.subagents.updated': {
    turnId: string;
    activeSubagents: ThreadSubagentDto[];
  };
  'thread.goal.updated': {
    turnId?: string | null;
    goal: ThreadGoalDto | null;
    goalHistory: ThreadGoalDto[];
  };
  'thread.goal.cleared': {
    goalHistory: ThreadGoalDto[];
  };
  'thread.turn.token.updated': {
    turnId: string;
    tokenUsage: ThreadTurnTokenUsageDto;
    priceEstimate: ThreadTurnPriceEstimateDto | null;
    model?: string | null;
    reasoningEffort?: ReasoningEffortDto | null;
    reasoningEffortAvailable?: boolean | null;
  };
  'thread.turn.started': {
    turnId: string;
  };
  'thread.item.started': {
    turnId: string;
    item: ThreadHistoryItemDto;
  };
  'thread.item.completed': {
    turnId: string;
    item: ThreadHistoryItemDto;
  };
  'thread.plan.updated': {
    turnId: string;
    explanation: string | null;
    plan: ThreadLivePlanDto['plan'];
  };
  'thread.request.created': {
    request: ThreadActionRequestDto;
  };
  'thread.request.resolved': {
    requestId: string;
  };
  'thread.output.delta': {
    turnId: string;
    itemId: string;
    sequence: number;
    delta: string;
    text?: string;
    createdAt?: string | null;
  };
  'thread.turn.completed': {
    turnId: string;
    status: ThreadTurnDto['status'];
    error: string | null;
  };
  'thread.persistence.failed': { message: string };
  'thread.turn.failed': {
    turnId: string;
    error: string | null;
    willRetry?: boolean;
  };
}

export type ThreadEventEnvelope = {
  [Type in keyof ThreadEventPayloadMap]: {
    type: Type;
    threadId: string;
    timestamp: string;
    payload: ThreadEventPayloadMap[Type];
  };
}[keyof ThreadEventPayloadMap];

export interface ShellEventPayloadMap {
  'shell.connected': {
    viewerId: string;
  };
  'shell.status': {
    threadId: string;
    state: ShellStatusDto;
    viewerId?: string;
  };
  'shell.output': {
    data: string;
    replace?: boolean;
    cursorX?: number;
    cursorY?: number;
    paneHeight?: number;
    cwdBaseName?: string;
    envPrefix?: string;
    isCommandRunning?: boolean;
  };
  'shell.detached': {
    threadId: string;
    state: Extract<ShellStatusDto, 'detached'>;
    viewerId: string;
    reason?: string;
  };
  'shell.exited': {
    threadId: string;
    state: Extract<ShellStatusDto, 'exited' | 'not_found'>;
  };
  'shell.error': {
    code: string;
    message: string;
  };
}

export type ShellEventEnvelope = {
  [Type in keyof ShellEventPayloadMap]: {
    type: Type;
    shellId: string;
    timestamp: string;
    payload: ShellEventPayloadMap[Type];
  };
}[keyof ShellEventPayloadMap];

export interface SupervisorConnectedEnvelope {
  type: 'supervisor.connected';
  timestamp: string;
}

export interface SupervisorPongEnvelope {
  type: 'supervisor.pong';
  timestamp: string;
  payload: {
    requestTimestamp: string | null;
  };
}

export type SupervisorSocketServerEnvelope =
  | SupervisorConnectedEnvelope
  | SupervisorPongEnvelope
  | ThreadEventEnvelope
  | ShellEventEnvelope;

export type SupervisorSocketClientEnvelope =
  | {
      type: 'supervisor.ping';
      timestamp: string;
    }
  | {
      type: 'shell.attach';
      shellId: string;
      cols: number;
      rows: number;
    }
  | {
      type: 'shell.detach';
      shellId: string;
      viewerId: string;
    }
  | {
      type: 'shell.input';
      shellId: string;
      viewerId: string;
      data: string;
    }
  | {
      type: 'shell.resize';
      shellId: string;
      viewerId: string;
      cols: number;
      rows: number;
    }
  | {
      type: 'shell.clear';
      shellId: string;
      viewerId: string;
    };


/** Text can arrive as a shorter summary; operation lifecycle must still advance. */
export function mergeThreadHistoryItem(
  current: ThreadHistoryItemDto | undefined,
  incoming: ThreadHistoryItemDto,
): ThreadHistoryItemDto {
  if (!current || current.kind !== incoming.kind) return incoming;
  const terminal = (status: string | null | undefined) =>
    ['completed', 'failed', 'interrupted', 'cancelled', 'canceled'].includes(status?.toLowerCase() ?? '');
  const richer = (previous: string | null | undefined, next: string | null | undefined) =>
    (previous?.length ?? 0) > (next?.length ?? 0) ? previous : next ?? previous;
  return {
    ...current,
    ...incoming,
    text: richer(current.text, incoming.text) ?? '',
    ...(current.detailText != null || incoming.detailText != null
      ? { detailText: richer(current.detailText, incoming.detailText) ?? '' } : {}),
    ...(current.previewText != null || incoming.previewText != null
      ? { previewText: richer(current.previewText, incoming.previewText) ?? '' } : {}),
    ...(current.status != null || incoming.status != null
      ? { status: terminal(current.status) && !terminal(incoming.status)
        ? current.status : incoming.status ?? current.status } : {}),
    ...(current.sequence != null || incoming.sequence != null
      ? { sequence: incoming.sequence ?? current.sequence } : {}),
  };
}
export type { DeviceMetricsDto, MemoryMetricsDto, GpuMetricsDto, PowerReadingDto, TemperatureReadingDto } from './deviceMetrics';
export type {
  HarnessBusyReason,
  HarnessLoadReason,
  HarnessProcessDto,
  HarnessProcessesDto,
  HarnessProcessRole,
  HarnessResourceSettingsDto,
  LoadedThreadDto,
  ProcessMemoryMetric,
  ReleaseThreadInput,
  ReleaseThreadResultDto,
  ThreadViewMessage,
} from './harnessResources';

export type { AutomationTrigger, AutomationCondition, AutomationCommandSpec, AutomationAction, AutomationDefinition, AutomationDto, AutomationRunDto } from "./automation";
