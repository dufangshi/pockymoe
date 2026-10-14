import { appPathname, appUrl } from './webBase';
import { deviceRoute } from './relayTransportCrypto';
import { markOwnedDevices } from './accountTransport';
import { sharingMutation, type SharingInput } from './signedSharing';
import { relayOrigin, relayUrl, relaySocketUrl, relayFetchInit } from './relayOrigin';
import { logoutAccount } from './accountVaultState';
import { startAccountLogin, setAccountSession, cancelAccountLogin, rememberAccountLogin, type AccountSessionState } from './accountAuth';
import { confirmWorkspaceDocumentLeave, type WorkspaceDocumentSnapshot, type WorkspaceDocumentSaveInput, type WorkspaceSaveReceipt } from "@pockymoe/thread-ui";
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { encryptedBrowserFetch, encryptedRelaySocket } from './relayTransport';
import type {
  ApplyProviderHostConfigArchiveResultDto,
  AgentBackendDto,
  AgentBackendIdDto,
  AgentCapabilitySnapshotDto,
  HarnessProcessesDto,
  HarnessResourceSettingsDto,
  ReleaseThreadInput,
  ReleaseThreadResultDto,
  AgentSubscriptionUsageDto,
  ApiErrorShape,
  AuthLoginResultDto,
  AuthSessionDto,
  CreateRelayAccessGrantInput,
  RelayAdminSummaryDto,
  RelayAccessGrantDto,
  RelayCreateDeviceResultDto,
  RelayLoginResultDto,
  RelayPortalSummaryDto,
  RelayRegisterResultDto,
  RelayRegistrationSettingsDto,
  RelaySessionDto,
  RelaySessionShareDto,
  UpdateRelayAccessGrantInput,
  UpdateRelaySessionShareInput,
  RelayUserDto,
  ProviderHostConfigArchiveDto,
  ProviderHostFileDto,
  CreateProviderHostConfigArchiveInput,
  CreateRelaySessionShareInput,
  CreateThreadInput,
  CreateThreadHookInput,
  TrustThreadHookInput,
  ForkThreadInput,
  RelayEffectiveAccessDto,
  CreateWorkspaceInput,
  HealthDto,
  ImportThreadCandidateDto,
  ImportThreadInput,
  ImportPluginInput,
  InterruptTurnInput,
  ModelOptionDto,
  PluginDto,
  RespondThreadActionRequestInput,
  ResumeThreadInput,
  RuntimeConfigDto,
  SendThreadPromptInput,
  ShellEventEnvelope,
  ShellSessionDto,
  SupervisorConnectedEnvelope,
  SupervisorSocketClientEnvelope,
  SupervisorSocketServerEnvelope,
  ThreadDetailDto,
  ThreadTurnDto,
  ThreadExportTurnOptionsDto,
  ThreadHistoryItemDetailDto,
  ThreadWorkspaceFilePreviewDto,
  ThreadWorkspaceTreeNodeDto,
  ThreadWorkspaceUploadResultDto,
  ThreadGoalDto,
  ThreadHooksDto,
  UntrustThreadHookInput,
  UpdateThreadGoalInput,
  UpdateThreadHookInput,
  ThreadMcpServersDto,
  ThreadSkillsDto,
  ThreadDto,
  ThreadEventEnvelope,
  ThreadForkResultDto,
  ThreadForkTurnOptionDto,
  WorkspaceFileDto,
  ThreadShellStateDto,
  RenameProviderHostConfigArchiveInput,
  UpdateShellInput,
  UpdateThreadSettingsInput,
  UpdateThreadInput,
  UpdateProviderHostFileInput,
  UpdatePluginInput,
  UpdateWorkspaceSettingsInput,
  UpdateWorkspaceInput,
  UpdateWorkspaceFavoriteInput,
  WorkspaceDto,
  WorkspaceSettingsDto,
} from '@pockymoe/shared';
export type { PromptAttachmentUpload } from '@pockymoe/thread-ui';
import type { PromptAttachmentUpload } from '@pockymoe/thread-ui';
import {
  currentRelayDeviceIdFromPath,
  currentThreadIdFromPath,
} from './relayRoutes';

export class ApiError extends Error {
  constructor(
    public readonly statusCode: number,
    public readonly payload: ApiErrorShape,
  ) {
    super(payload.message);
  }
}

const AUTH_TOKEN_STORAGE_KEY = 'remote-codex-auth-token';
const RELAY_TOKEN_STORAGE_KEY = 'remote-codex-relay-token';
const RELAY_ADMIN_TOKEN_STORAGE_KEY = 'remote-codex-relay-admin-token';
const RELAY_MODE_STORAGE_KEY = 'remote-codex-relay-mode';
const RELAY_DEVICE_STORAGE_KEY = 'remote-codex-relay-device-id';
const RELAY_THREAD_STORAGE_KEY = 'remote-codex-relay-thread-id';
type RequestAuthMode = 'default' | 'relay-admin' | 'none';

declare global {
  interface Window {
    __POCKYMOE_BOOTSTRAP__?: {
      mode?: 'local' | 'server' | 'relay';
    };
  }
}

function readStoredAuthToken() {
  if (typeof window === 'undefined') {
    return null;
  }

  return window.localStorage.getItem(AUTH_TOKEN_STORAGE_KEY);
}

export function setStoredAuthToken(token: string | null) {
  if (typeof window === 'undefined') {
    return;
  }

  if (token) {
    window.localStorage.setItem(AUTH_TOKEN_STORAGE_KEY, token);
    return;
  }

  window.localStorage.removeItem(AUTH_TOKEN_STORAGE_KEY);
}

// Relay credentials live only in HttpOnly cookies. Clear legacy bearer copies
// during migration; never place long-lived credentials in storage or asset URLs.
function readStoredRelayToken(): null {
  if (typeof window !== 'undefined') window.localStorage.removeItem(RELAY_TOKEN_STORAGE_KEY);
  return null;
}

function readStoredRelayAdminToken(): null {
  if (typeof window !== 'undefined') window.localStorage.removeItem(RELAY_ADMIN_TOKEN_STORAGE_KEY);
  return null;
}

export function setStoredRelayToken(_token: string | null) { readStoredRelayToken(); }
export function setStoredRelayAdminToken(_token: string | null) { readStoredRelayAdminToken(); }

function relayModeEnabled() {
  if (typeof window === 'undefined') {
    return false;
  }
  return (
    window.__POCKYMOE_BOOTSTRAP__?.mode === 'relay' ||
    import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true' ||
    !!import.meta.env.VITE_POCKYMOE_RELAY_ORIGIN ||
    appPathname(window.location.pathname).startsWith('/relay-portal') ||
    appPathname(window.location.pathname).startsWith('/relay-admin') ||
    appPathname(window.location.pathname).startsWith('/relay-account') ||
    appPathname(window.location.pathname).startsWith('/relay-devices') ||
    appPathname(window.location.pathname).startsWith('/relay-guide') ||
    window.location.search.includes('relay=1') ||
    window.localStorage.getItem(RELAY_MODE_STORAGE_KEY) === 'true'
  );
}

export function enableRelayMode() {
  if (typeof window === 'undefined') {
    return;
  }
  window.localStorage.setItem(RELAY_MODE_STORAGE_KEY, 'true');
}

export function relayModeActive() {
  return relayModeEnabled();
}

export function readSelectedRelayDeviceId() {
  if (typeof window === 'undefined') {
    return null;
  }
  return (
    currentRelayDeviceIdFromPath() ??
    window.localStorage.getItem(RELAY_DEVICE_STORAGE_KEY)
  );
}

export function setSelectedRelayDeviceId(deviceId: string | null) {
  if (typeof window === 'undefined') {
    return;
  }
  if (deviceId !== readSelectedRelayDeviceId() && !confirmWorkspaceDocumentLeave()) throw new Error(translate('files.keepEditing'));
  if (deviceId) {
    window.localStorage.setItem(RELAY_DEVICE_STORAGE_KEY, deviceId);
    return;
  }
  window.localStorage.removeItem(RELAY_DEVICE_STORAGE_KEY);
}

export function readSelectedRelayThreadId() {
  if (typeof window === 'undefined') {
    return null;
  }
  return (
    currentThreadIdFromPath() ??
    window.localStorage.getItem(RELAY_THREAD_STORAGE_KEY)
  );
}

export function setSelectedRelayThreadId(threadId: string | null) {
  if (typeof window === 'undefined') {
    return;
  }
  if (threadId) {
    window.localStorage.setItem(RELAY_THREAD_STORAGE_KEY, threadId);
    return;
  }
  window.localStorage.removeItem(RELAY_THREAD_STORAGE_KEY);
}

function apiPath(path: string, deviceId?: string | null) {
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(path)) return path;
  // Normalize logical inputs before selecting a device. Keep the result relative
  // so a mount named /api/ or /relay/ cannot be mistaken for part of this path.
  const logical = new URL(path, globalThis.location?.origin ?? 'http://localhost');
  path = `${logical.pathname}${logical.search}${logical.hash}`;
  // Explicit targets are captured by each pane; never mutate global navigation.
  if (deviceId !== undefined) {
    return deviceId && deviceId !== 'local' && path.startsWith('/api/')
      ? `relay/devices/${encodeURIComponent(deviceId)}${path}` : path.slice(1);
  }
  if (relayModeEnabled() && path.startsWith('/api/')) {
    const selectedDevice = readSelectedRelayDeviceId();
    return selectedDevice
      ? `relay/devices/${encodeURIComponent(selectedDevice)}${path}`
      : `relay${path}`;
  }
  return path.slice(1);
}

export function buildApiUrl(path: string) {
  return relayUrl(apiPath(path));
}

function buildBrowserMediaUrl(path: string, deviceId?: string | null) {
  const resolvedPath = apiPath(path, deviceId);
  if (typeof window === 'undefined') {
    return resolvedPath;
  }

  const url = new URL(relayUrl(resolvedPath), window.location.origin);
  const relayToken = readStoredRelayToken();
  const authToken = readStoredAuthToken();
  if (relayModeEnabled() && relayToken) {
    url.searchParams.set('relaySession', relayToken);
  } else if (authToken) {
    url.searchParams.set('token', authToken);
  }
  return url.origin === window.location.origin ? `${url.pathname}${url.search}${url.hash}` : url.toString();
}

// These wrappers keep every operation (including retries/downloads) on the pane's device.
function deviceRequest<T>(deviceId: string | null | undefined, input: RequestInfo, init?: RequestInit, threadScope?: string) {
  return request<T>(input, init, { deviceId, threadScope });
}
function deviceDownload(deviceId: string | null | undefined, input: RequestInfo | URL, init?: RequestInit) {
  return downloadFile(input, init, deviceId);
}
function buildDeviceApiUrl(deviceId: string | null | undefined, path: string) {
  return relayUrl(apiPath(path, deviceId));
}
function buildDeviceMediaUrl(deviceId: string | null | undefined, path: string) {
  return buildBrowserMediaUrl(path, deviceId);
}

export interface FileDownloadResult {
  blob: Blob;
  filename: string;
}

function apiErrorCodeForStatus(status: number): ApiErrorShape['code'] {
  if (status === 400) {
    return 'bad_request';
  }

  if (status === 401) {
    return 'unauthorized';
  }

  if (status === 403) {
    return 'forbidden';
  }

  if (status === 404) {
    return 'not_found';
  }

  if (status === 409) {
    return 'conflict';
  }

  if (status === 429 || status === 503) {
    return 'service_unavailable';
  }

  return 'internal_error';
}

function fallbackErrorMessage(status: number, statusText?: string) {
  const label = statusText?.trim();
  const suffix = label ? `${status} ${label}` : `${status}`;

  if (status === 429) {
    return translate("workbench.tooManyRequests", { value1: suffix });
  }

  if (status === 504) return translate("workbench.theRequestTimedOutBeforeDeliveryCould");

  if (status === 503) {
    return translate("workbench.upstreamServiceUnavailable", { value1: suffix });
  }

  return translate("workbench.requestFailed", { value1: suffix });
}

function normalizedApiErrorPayload(
  response: Response,
  payload: Partial<ApiErrorShape> | null | undefined,
  fallbackMessage: string,
): ApiErrorShape {
  const message =
    String(payload?.code) === 'grant_denied' ? translate('sharing.grantDenied') :
    String(payload?.code) === 'owner_required' ? translate('sharing.ownerRequired') :
    typeof payload?.message === 'string' && payload.message.trim()
      ? payload.message.trim()
      : fallbackMessage;
  const details =
    payload?.details && typeof payload.details === 'object'
      ? payload.details
      : undefined;

  return {
    code: payload?.code ?? apiErrorCodeForStatus(response.status),
    message,
    ...(details ? { details } : {}),
  };
}

async function readApiErrorPayload(response: Response): Promise<ApiErrorShape> {
  const fallbackMessage = fallbackErrorMessage(
    response.status,
    response.statusText,
  );
  const contentType = response.headers?.get?.('content-type') ?? '';
  const readJsonPayload = async () =>
    normalizedApiErrorPayload(
      response,
      (await response.json()) as Partial<ApiErrorShape>,
      fallbackMessage,
    );

  if (contentType.includes('application/json')) {
    try {
      return await readJsonPayload();
    } catch {
      return normalizedApiErrorPayload(response, null, fallbackMessage);
    }
  }

  try {
    if (typeof response.text !== 'function') {
      try {
        return await readJsonPayload();
      } catch {
        return normalizedApiErrorPayload(response, null, fallbackMessage);
      }
    }

    const text = (await response.text()).trim();
    if (text.startsWith('{')) {
      try {
        return normalizedApiErrorPayload(
          response,
          JSON.parse(text) as Partial<ApiErrorShape>,
          fallbackMessage,
        );
      } catch {
        // Keep the raw response text visible below when it is not valid JSON.
      }
    }

    return normalizedApiErrorPayload(
      response,
      text && !contentType.includes('text/html') && !/<(?:!doctype|html|head|body)\b/i.test(text)
        ? { message: `${fallbackMessage}\n${text.slice(0, 500)}` } : null,
      fallbackMessage,
    );
  } catch {
    try {
      return await readJsonPayload();
    } catch {
      return normalizedApiErrorPayload(response, null, fallbackMessage);
    }
  }
}

function fetchApi(path: string, init: RequestInit, deviceId?: string | null, threadScope?: string) {
  const url = apiPath(path, deviceId);
  const target = relayUrl(url);
  const endpoint = new URL(target, window.location.origin);
  const requestInit = relayFetchInit(target, init);
  return endpoint.origin === relayOrigin() && deviceRoute(endpoint) ? encryptedBrowserFetch(target, requestInit, threadScope) : fetch(target, requestInit);
}

export async function request<T>(
  input: RequestInfo,
  init?: RequestInit,
  options: { auth?: RequestAuthMode; deviceId?: string | null | undefined; threadScope?: string | undefined } = {},
): Promise<T> {
  const headers = new Headers(init?.headers);
  if (
    init?.body !== undefined &&
    !(init.body instanceof FormData) &&
    !headers.has('Content-Type')
  ) {
    headers.set('Content-Type', 'application/json');
  }

  const requestInit = withAuthInit(
    {
      ...init,
      headers,
    },
    options.auth,
  );
  const targetPath = apiPath(String(input), options.deviceId);
  const response = await fetchApi(targetPath, requestInit, null, options.threadScope);
  if (!response.ok) {
    throw new ApiError(response.status, await readApiErrorPayload(response));
  }
  const text = await response.text();
  return (text ? JSON.parse(text) : {}) as T;
}

function fallbackDownloadFilename(input: RequestInfo | URL) {
  return String(input).includes('/exports/html') ? 'pockymoe-transcript.html' : 'download';
}

function parseContentDispositionFilename(value: string | null) {
  if (!value) {
    return null;
  }

  const utf8Match = value.match(/filename\*=UTF-8''([^;]+)/i);
  if (utf8Match?.[1]) {
    try {
      return decodeURIComponent(utf8Match[1].trim());
    } catch {
      return utf8Match[1].trim();
    }
  }

  const quotedMatch = value.match(/filename="([^"]+)"/i);
  if (quotedMatch?.[1]) {
    return quotedMatch[1].trim();
  }

  const plainMatch = value.match(/filename=([^;]+)/i);
  return plainMatch?.[1]?.trim() ?? null;
}

async function downloadFile(
  input: RequestInfo | URL,
  init?: RequestInit,
  deviceId?: string | null,
): Promise<FileDownloadResult> {
  const response = await fetchApi(String(input), withAuthInit(init), deviceId);

  if (!response.ok) {
    const payload = await readApiErrorPayload(response);
    throw new ApiError(response.status, payload);
  }

  const filename =
    parseContentDispositionFilename(
      response.headers.get('content-disposition'),
    ) ?? fallbackDownloadFilename(input);

  return {
    blob: await response.blob(),
    filename,
  };
}

function withAuthInit(
  init: RequestInit = {},
  authMode: RequestAuthMode = 'default',
): RequestInit {
  const headers = new Headers(init.headers);
  const relayMode = relayModeEnabled();
  if (authMode !== 'none' && !headers.has('Authorization')) {
    if (authMode === 'relay-admin') {
      headers.set('X-Remote-Codex-Auth-Realm', 'admin');
      const relayAdminToken = readStoredRelayAdminToken();
      if (relayAdminToken) {
        headers.set('Authorization', `Bearer ${relayAdminToken}`);
      }
    } else {
      const relayToken = readStoredRelayToken();
      const token = readStoredAuthToken();
      if (relayMode && relayToken) {
        headers.set('Authorization', `Bearer ${relayToken}`);
      } else if (!relayMode && token) {
        headers.set('Authorization', `Bearer ${token}`);
      }
    }
  }

  return {
    ...init,
    credentials: init.credentials ?? 'same-origin',
    headers,
  };
}

export interface SendThreadPromptRequestInput extends SendThreadPromptInput {
  attachments?: PromptAttachmentUpload[];
  delivery?: 'steer';
}

function normalizedUploadFileName(
  attachment: PromptAttachmentUpload,
  index: number,
) {
  const explicitName = attachment.originalName.trim();
  if (explicitName) {
    return explicitName;
  }

  const fileName = attachment.file.name.trim();
  if (fileName) {
    return fileName;
  }

  return attachment.kind === 'photo'
    ? `photo-${index + 1}.jpg`
    : `file-${index + 1}`;
}

export function fetchRuntimeConfig() {
  return request<RuntimeConfigDto>('api/config/runtime');
}

let workspaceActorScope = "local:owner";
export async function fetchAuthSession() {
  const result = await request<AuthSessionDto>('api/auth/session', {
    cache: 'no-store',
  });
  if (result.mode !== 'relay') workspaceActorScope = `local:${result.username??'owner'}`;
  return result;
}

export async function login(input: { username: string; password: string }) {
  const result = await request<AuthLoginResultDto>('api/auth/login', {
    method: 'POST',
    body: JSON.stringify(input),
  });
  setStoredAuthToken(result.token ?? null);
  return result;
}

export async function logout() {
  if (!confirmWorkspaceDocumentLeave()) throw new Error(translate('files.keepEditing'));
  const result = await request<AuthSessionDto>('api/auth/logout', {
    method: 'POST',
  });
  setStoredAuthToken(null);
  return result;
}

export async function fetchRelaySession() {
  const result = await request<RelaySessionDto>('relay/auth/session', {
    cache: 'no-store',
  });
  workspaceActorScope = `relay:${result.user?.id??'anonymous'}`;
  await setAccountSession(result.authenticated ? result.user?.id ?? null : null, (result as RelaySessionDto & { account?: AccountSessionState }).account ?? false);
  if (result.authenticated && result.user) await rememberAccountLogin(result.user);
  return result;
}

export async function relayLogin(input: {
  identifier: string;
  password: string;
}) {
  enableRelayMode();
  const result = await startAccountLogin(input.identifier, input.password);
  setStoredRelayToken(result.token ?? null);
  return result;
}

export function fetchRelayAdminSession() {
  enableRelayMode();
  return request<RelaySessionDto>(
    'relay/auth/session',
    {
      cache: 'no-store',
    },
    { auth: 'relay-admin' },
  );
}

export async function relayAdminLogin(input: {
  username: string;
  password: string;
}) {
  enableRelayMode();
  const result = await request<RelayLoginResultDto>(
    'relay/auth/login',
    {
      method: 'POST',
      body: JSON.stringify({
        identifier: input.username,
        password: input.password,
      }),
    },
    { auth: 'none' },
  );
  setStoredRelayAdminToken(result.token ?? null);
  return result;
}

export async function relayAdminLogout() {
  const result = await request<RelaySessionDto>('relay/auth/logout', { method: 'POST' }, { auth: 'relay-admin' });
  setStoredRelayAdminToken(null);
  return result;
}

export async function relayLogout() {
  if (!confirmWorkspaceDocumentLeave()) throw new Error(translate('files.keepEditing'));
  try {
    return await request<RelaySessionDto>('relay/auth/logout', { method: 'POST' });
  } finally {
    cancelAccountLogin(); await logoutAccount(); setStoredRelayToken(null);
    setSelectedRelayDeviceId(null); setSelectedRelayThreadId(null);
  }
}

export async function fetchRelayPortal() {
  const summary = await request<RelayPortalSummaryDto>('relay/portal');
  await markOwnedDevices(summary.devices.filter(d => d.ownerUserId === summary.user.id).map(d => d.id));
  return summary;
}

export function fetchRelayAccess(input: {
  deviceId: string;
  threadId?: string | null;
  workspaceId?: string | null;
}) {
  const params = new URLSearchParams({
    deviceId: input.deviceId,
  });
  if (input.threadId) {
    params.set('threadId', input.threadId);
  }
  if (input.workspaceId) {
    params.set('workspaceId', input.workspaceId);
  }
  return request<RelayEffectiveAccessDto>(`relay/access?${params.toString()}`);
}

export function createRelayDevice(input: { name: string }) {
  return request<RelayCreateDeviceResultDto>('relay/devices', {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function deleteRelayDevice(deviceId: string) {
  return request<{ id: string }>(
    `relay/devices/${encodeURIComponent(deviceId)}`,
    {
      method: 'DELETE',
    },
  );
}

export function updateRelayAccount(input: { username?: string; avatarUrl?: string | null }) {
  return request<RelayUserDto>('relay/account', {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function createRelayShare(input: CreateRelaySessionShareInput) {
  return sharingMutation('share', 'create', undefined, () => request<RelaySessionShareDto>('relay/shares', {
    method: 'POST',
    body: JSON.stringify(input),
  }), input);
}

export function updateRelayShare(
  shareId: string,
  input: UpdateRelaySessionShareInput,
  dialogContext?: SharingInput,
) {
  return sharingMutation('share', 'edit', shareId, () => request<RelaySessionShareDto>(
    `relay/shares/${encodeURIComponent(shareId)}`,
    {
      method: 'PATCH',
      body: JSON.stringify(input),
    },
  ), { ...dialogContext, ...input });
}

export function revokeRelayShare(shareId: string) {
  return sharingMutation('share', 'revoke', shareId, () => request<RelaySessionShareDto>(
    `relay/shares/${encodeURIComponent(shareId)}`,
    {
      method: 'DELETE',
    },
  ));
}

export function createRelayGrant(input: CreateRelayAccessGrantInput) {
  return sharingMutation('grant', 'create', undefined, () => request<RelayAccessGrantDto>('relay/grants', {
    method: 'POST',
    body: JSON.stringify(input),
  }), input);
}

export function updateRelayGrant(
  grantId: string,
  input: UpdateRelayAccessGrantInput,
  dialogContext?: SharingInput,
) {
  return sharingMutation('grant', 'edit', grantId, () => request<RelayAccessGrantDto>(
    `relay/grants/${encodeURIComponent(grantId)}`,
    {
      method: 'PATCH',
      body: JSON.stringify(input),
    },
  ), { ...dialogContext, ...input });
}

export function revokeRelayGrant(grantId: string) {
  return sharingMutation('grant', 'revoke', grantId, () => request<RelayAccessGrantDto>(
    `relay/grants/${encodeURIComponent(grantId)}`,
    {
      method: 'DELETE',
    },
  ));
}

export function fetchRelayAdmin(days?: number) {
  const query = days ? `?days=${encodeURIComponent(String(days))}` : '';
  return request<RelayAdminSummaryDto>(`relay/admin${query}`, undefined, {
    auth: 'relay-admin',
  });
}

export function updateRelayRegistrationSettings(
  input: Partial<RelayRegistrationSettingsDto>,
) {
  return request<{
    registrationEnabled: boolean;
    settings: RelayRegistrationSettingsDto;
  }>(
    'relay/admin/settings/registration',
    {
      method: 'PATCH',
      body: JSON.stringify(input),
    },
    { auth: 'relay-admin' },
  );
}

export function setRelayUserEnabled(userId: string, enabled: boolean) {
  return request<RelayUserDto>(
    `relay/admin/users/${encodeURIComponent(userId)}`,
    {
      method: 'PATCH',
      body: JSON.stringify({ enabled }),
    },
    { auth: 'relay-admin' },
  );
}

export function deleteRelayAdminUser(userId: string) {
  return request<{ id: string }>(
    `relay/admin/users/${encodeURIComponent(userId)}`,
    {
      method: 'DELETE',
    },
    { auth: 'relay-admin' },
  );
}

export function resetRelayAdminUserPassword(userId: string, password: string) {
  return request<RelayUserDto>(
    `relay/admin/users/${encodeURIComponent(userId)}/reset-password`,
    {
      method: 'POST',
      body: JSON.stringify({ password }),
    },
    { auth: 'relay-admin' },
  );
}

export function approveRelayRegistration(requestId: string) {
  return request<RelayUserDto>(
    `relay/admin/registrations/${encodeURIComponent(requestId)}/approve`,
    {
      method: 'POST',
    },
    { auth: 'relay-admin' },
  );
}

export function rejectRelayRegistration(requestId: string) {
  return request<{ id: string }>(
    `relay/admin/registrations/${encodeURIComponent(requestId)}/reject`,
    {
      method: 'POST',
    },
    { auth: 'relay-admin' },
  );
}

export function fetchWorkspaceSettings() {
  return request<WorkspaceSettingsDto>('api/config/workspace-settings', {
    cache: 'no-store',
  });
}

export function updateWorkspaceSettings(input: UpdateWorkspaceSettingsInput) {
  return request<WorkspaceSettingsDto>('api/config/workspace-settings', {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function fetchAgentBackends() {
  return request<AgentBackendDto[]>('api/agent-runtimes', {
    cache: 'no-store',
  });
}

export function fetchAgentBackendStatus(provider: AgentBackendIdDto) {
  return request<AgentBackendDto>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/status`,
    {
      cache: 'no-store',
    },
  );
}

export function fetchAgentSubscriptionUsage(provider: AgentBackendIdDto, agentId?: string | null) {
  return request<{ usage: AgentSubscriptionUsageDto | null; unavailable?: boolean }>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/subscription-usage${agentId ? `?agentId=${encodeURIComponent(agentId)}` : ""}`,
    { cache: 'no-store' },
  );
}

export function installOrUpdateAgentBackend(
  provider: AgentBackendIdDto,
  action: 'install' | 'update',
  modelId?: string,
) {
  return request<AgentBackendDto>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/install${modelId ? `?agentId=${encodeURIComponent(modelId)}` : ''}`,
    {
      method: 'POST',
      body: JSON.stringify({
        action,
        ...(modelId ? { modelId } : {}),
      }),
    },
  );
}

export function fetchAgentBackendModels(provider: AgentBackendIdDto) {
  return fetchAgentBackendModelsFor(provider);
}

export function fetchAgentBackendModelsFor(
  provider: AgentBackendIdDto,
  options: { agentId?: string | null; cwd?: string | null } = {},
) {
  const query = new URLSearchParams();
  if (options.agentId) {
    query.set('agentId', options.agentId);
  }
  if (options.cwd) {
    query.set('cwd', options.cwd);
  }
  const suffix = query.size > 0 ? `?${query.toString()}` : '';
  return request<ModelOptionDto[]>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/models${suffix}`,
    {
      cache: 'no-store',
    },
  );
}

export function fetchAgentBackendAgents(provider: AgentBackendIdDto) {
  return request<ModelOptionDto[]>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/agents`,
    { cache: 'no-store' },
  );
}

/** `load: false` reads a thread's capabilities without starting its harness. */
export function fetchThreadCapabilitySnapshot(threadId: string,
  deviceId?: string | null,
  options: { load?: boolean } = {},
) {
  const query = options.load === false ? '?load=false' : '';
  return deviceRequest<AgentCapabilitySnapshotDto>(deviceId, `api/threads/${encodeURIComponent(threadId)}/capabilities${query}`, { cache: 'no-store' });
}

/** Ends a thread's harness processes; `interrupt` first stops a running turn. */
export function releaseThread(threadId: string, input: ReleaseThreadInput = {},
  deviceId?: string | null,
) {
  return deviceRequest<ReleaseThreadResultDto>(deviceId, `api/threads/${encodeURIComponent(threadId)}/release`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function fetchHarnessProcesses(signal?: AbortSignal) {
  return request<HarnessProcessesDto>('api/device/processes', { cache: 'no-store', ...(signal ? { signal } : {}) });
}

export function fetchHarnessResourceSettings(deviceId?: string | null) {
  return deviceRequest<HarnessResourceSettingsDto>(deviceId, 'api/harness-resources/settings', { cache: 'no-store' });
}

export function updateHarnessResourceSettings(input: HarnessResourceSettingsDto,
  deviceId?: string | null,
) {
  return deviceRequest<HarnessResourceSettingsDto>(deviceId, 'api/harness-resources/settings', {
    method: 'PUT',
    body: JSON.stringify(input),
  });
}

export function postThreadHarnessAction<T>(threadId: string,
  action: unknown,
  deviceId?: string | null,
) {
  return deviceRequest<T>(deviceId, `api/threads/${encodeURIComponent(threadId)}/harness`, {
    method: 'POST',
    body: JSON.stringify(action),
  });
}

/** Harness-specific creation options (DSH run modes); null for other harnesses. */
export function fetchAgentHarnessCatalog<T>(
  provider: AgentBackendIdDto,
  options: { agentId?: string | null; cwd?: string | null } = {},
) {
  const query = new URLSearchParams();
  if (options.agentId) query.set('agentId', options.agentId);
  if (options.cwd) query.set('cwd', options.cwd);
  const suffix = query.size > 0 ? `?${query.toString()}` : '';
  return request<T | null>(
    `api/agent-runtimes/${encodeURIComponent(provider)}/harness${suffix}`,
    { cache: 'no-store' },
  );
}

/**
 * Browser address of a thread's native DSH console (a loopback port on the
 * device). Relay pages open it through an owner-only port preview; local pages
 * reach it only from the device itself.
 */
export async function dshConsoleUrl(
  threadId: string,
  target: { port: number; path: string; mappingId?: string; leaseId?: string; expiresAt?: number },
  deviceId?: string | null,
) {
  if (!deviceId || deviceId === 'local') {
    const host = window.location.hostname;
    if (!['localhost', '127.0.0.1', '[::1]'].includes(host)) {
      throw new Error(translate('settings.dshConsoleLoopbackOnly'));
    }
    // Same site as this page, or the browser withholds DSH's SameSite=Strict
    // login cookie on the token redirect. The proxy listens on IPv4 loopback.
    return `http://${host === '[::1]' ? '127.0.0.1' : host}:${target.port}${target.path}`;
  }
  const device = `relay/devices/${encodeURIComponent(deviceId)}`;
  // The device maps its console port (and drops stopped consoles). Older
  // devices leave it to the browser: one mapping per thread.
  let mappingId = target.mappingId;
  if (!mappingId) {
    const label = `DSH console ${threadId.slice(0, 8)}`;
    type Mapping = { id: string; port: number; label: string };
    const { mappings } = await request<{ mappings: Mapping[] }>(`${device}/api/port-mappings`);
    await Promise.all(mappings
      .filter((mapping) => mapping.label === label && mapping.port !== target.port)
      .map((mapping) => request(`${device}/api/port-mappings/${mapping.id}`, { method: 'DELETE' })));
    mappingId = (mappings.find((entry) => entry.port === target.port)
      ?? await request<Mapping>(`${device}/api/port-mappings`, {
        method: 'POST',
        body: JSON.stringify({ port: target.port, label }),
      })).id;
  }
  const { openDevicePortPreview } = await import('./portPreview');
  const { url } = await openDevicePortPreview(deviceId, mappingId, target.path, undefined, target.leaseId ? { leaseId: target.leaseId, expiresAt: target.expiresAt } : undefined);
  return url;
}

export function fetchThreadModels(threadId: string,
  deviceId?: string | null,
) {
  return deviceRequest<ModelOptionDto[]>(deviceId, `api/threads/${encodeURIComponent(threadId)}/models`, {cache:'no-store'});
}
export function fetchThreadGroup(threadId: string) {
  return request<ThreadDto[]>(`api/threads/${encodeURIComponent(threadId)}/group`, {cache:'no-store'});
}

export function fetchProviderHostFile(
  provider: AgentBackendIdDto,
  name: string,
) {
  return request<ProviderHostFileDto>(
    `api/config/providers/${encodeURIComponent(provider)}/files/${encodeURIComponent(name)}`,
    {
      cache: 'no-store',
    },
  );
}

export function updateProviderHostFile(
  provider: AgentBackendIdDto,
  name: string,
  input: UpdateProviderHostFileInput,
) {
  return request<ProviderHostFileDto>(
    `api/config/providers/${encodeURIComponent(provider)}/files/${encodeURIComponent(name)}`,
    {
      method: 'PATCH',
      body: JSON.stringify(input),
    },
  );
}

export function fetchProviderHostConfigArchives(provider: AgentBackendIdDto) {
  return request<ProviderHostConfigArchiveDto[]>(
    `api/config/providers/${encodeURIComponent(provider)}/archives`,
    {
      cache: 'no-store',
    },
  );
}

export function createProviderHostConfigArchive(
  provider: AgentBackendIdDto,
  input: CreateProviderHostConfigArchiveInput = {},
) {
  return request<ProviderHostConfigArchiveDto>(
    `api/config/providers/${encodeURIComponent(provider)}/archives`,
    {
      method: 'POST',
      body: JSON.stringify(input),
    },
  );
}

export function renameProviderHostConfigArchive(
  provider: AgentBackendIdDto,
  id: string,
  input: RenameProviderHostConfigArchiveInput,
) {
  return request<ProviderHostConfigArchiveDto>(
    `api/config/providers/${encodeURIComponent(provider)}/archives/${encodeURIComponent(id)}`,
    {
      method: 'PATCH',
      body: JSON.stringify(input),
    },
  );
}

export function applyProviderHostConfigArchive(
  provider: AgentBackendIdDto,
  id: string,
) {
  return request<ApplyProviderHostConfigArchiveResultDto>(
    `api/config/providers/${encodeURIComponent(provider)}/archives/${encodeURIComponent(id)}/apply`,
    {
      method: 'POST',
    },
  );
}

export function fetchSupervisorHealth() {
  return request<HealthDto>('/healthz', {
    cache: 'no-store',
  });
}

export function fetchWorkspaces() {
  return request<WorkspaceDto[]>('api/workspaces');
}

export function fetchLinkedFile(threadId: string, path: string,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadWorkspaceTreeNodeDto>(deviceId, `api/threads/${encodeURIComponent(threadId)}/linked-files/stat?${new URLSearchParams({path})}`, {cache: 'no-store'});
}
export function fetchLinkedFilePreview(threadId: string, input: {path: string; offset?: number; limit?: number},
  deviceId?: string | null,
) {
  const params = new URLSearchParams({path: input.path, offset: String(input.offset ?? 0), limit: String(input.limit ?? 24000)});
  return deviceRequest<ThreadWorkspaceFilePreviewDto>(deviceId, `api/threads/${encodeURIComponent(threadId)}/linked-files/preview?${params}`, {cache: 'no-store'});
}
export async function downloadLinkedFile(threadId: string, path: string,
  deviceId?: string | null,
  signal?: AbortSignal,
) {
  const result = await deviceDownload(deviceId, `api/threads/${encodeURIComponent(threadId)}/linked-files/raw?${new URLSearchParams({path})}`, signal ? { signal } : undefined);
  return { ...result, filename: path.replace(/\\/g, '/').split('/').pop() || 'download' };
}

export function buildLinkedFileUrl(threadId: string, path: string,
  deviceId?: string | null,
) {
  return buildDeviceMediaUrl(deviceId, `api/threads/${encodeURIComponent(threadId)}/linked-files/raw?${new URLSearchParams({path})}`);
}

export function fetchWorkspaceFileTree(
  workspaceId: string,
  input: { path?: string | null } = {},
  deviceId?: string | null,
) {
  const params = new URLSearchParams();
  if (input.path) {
    params.set('path', input.path);
  }
  const suffix = params.size > 0 ? `?${params.toString()}` : '';
  return deviceRequest<ThreadWorkspaceTreeNodeDto>(deviceId,
    `api/workspaces/${encodeURIComponent(workspaceId)}/files/tree${suffix}`,
    {
      cache: 'no-store',
    },
  );
}

export function fetchWorkspaceFilePreview(
  workspaceId: string,
  input: { path: string; offset?: number; limit?: number },
  deviceId?: string | null,
) {
  const params = new URLSearchParams({ path: input.path });
  if (input.offset !== undefined) {
    params.set('offset', String(input.offset));
  }
  if (input.limit !== undefined) {
    params.set('limit', String(input.limit));
  }

  return deviceRequest<ThreadWorkspaceFilePreviewDto>(deviceId,
    `api/workspaces/${encodeURIComponent(workspaceId)}/files/preview?${params.toString()}`,
    {
      cache: 'no-store',
    },
  );
}

export function buildWorkspaceRawFileUrl(
  workspaceId: string,
  input: { path: string },
  deviceId?: string | null,
) {
  const params = new URLSearchParams({ path: input.path });
  return buildDeviceApiUrl(deviceId,
    `api/workspaces/${encodeURIComponent(workspaceId)}/files/raw?${params.toString()}`,
  );
}

export function buildThreadImageAssetUrl(
  threadId: string,
  input: { path: string; variant?: 'thumbnail' | undefined },
  deviceId?: string | null,
) {
  const params = new URLSearchParams({ path: input.path });
  // Older Supervisors ignore the variant and send the original.
  if (input.variant) params.set('variant', input.variant);
  return buildDeviceMediaUrl(deviceId,
    `api/threads/${encodeURIComponent(threadId)}/assets/image?${params.toString()}`,
  );
}

export async function downloadThreadImage(threadId: string, path: string) {
  const result = await downloadFile(`api/threads/${encodeURIComponent(threadId)}/assets/image?${new URLSearchParams({ path })}`);
  return result.blob;
}

export function downloadWorkspaceFile(
  workspaceId: string,
  input: { path: string },
  deviceId?: string | null,
  signal?: AbortSignal,
) {
  const params = new URLSearchParams({ path: input.path });
  return deviceDownload(deviceId,
    `api/workspaces/${encodeURIComponent(workspaceId)}/files/download?${params.toString()}`,
    {
      cache: 'no-store',
      ...(signal ? { signal } : {}),
    },
  );
}

export function uploadWorkspaceFile(
  workspaceId: string,
  input: { file: File },
  deviceId?: string | null,
) {
  const formData = new FormData();
  formData.append('file', input.file, input.file.name);
  return deviceRequest<ThreadWorkspaceUploadResultDto>(deviceId,
    `api/workspaces/${encodeURIComponent(workspaceId)}/files/upload`,
    {
      method: 'POST',
      body: formData,
    },
  );
}

export function writeWorkspaceFile(
  workspaceId: string,
  input: { path: string; content: string },
) {
  return request<WorkspaceFileDto>(
    `api/workspaces/${encodeURIComponent(workspaceId)}/files`,
    {
      method: 'PUT',
      body: JSON.stringify(input),
    },
  );
}

export function fetchThreads(includeAgentThreads = false) {
  return request<ThreadDto[]>(`api/threads${includeAgentThreads ? '?includeAgentThreads=true' : ''}`);
}

export function renameWorkspaceNode(workspaceId: string, input: {fromPath: string; toPath: string},
  deviceId?: string | null,
) {
  return deviceRequest(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files/move`, {method:'PATCH',body:JSON.stringify(input)});
}
export function createWorkspaceFile(workspaceId: string, path: string,
  deviceId?: string | null,
) {
  return deviceRequest<{ path: string }>(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files`, {
    method: 'POST', body: JSON.stringify({ path }),
  });
}
export function deleteWorkspaceNode(workspaceId: string, path: string,
  deviceId?: string | null,
) {
  return deviceRequest(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files?${new URLSearchParams({path})}`, {method:'DELETE'});
}

export function fetchThreadDetail(
  id: string,
  options: { limit?: number; beforeTurnId?: string } = {},
  deviceId?: string | null,
) {
  const params = new URLSearchParams();
  params.set('view', 'summary');
  params.set('limit', String(options.limit ?? 3));
  // Compact items: tool detail loads on expand; older Supervisors ignore it.
  params.set('compact', '1');
  if (options.beforeTurnId) {
    params.set('beforeTurnId', options.beforeTurnId);
  }

  return deviceRequest<ThreadDetailDto>(deviceId,
    `api/threads/${id}?${params.toString()}`,
  );
}

export interface ConversationSearchMatch {
  turnId: string;
  itemId: string;
  role: string;
  text: string;
  createdAt?: string;
}
export interface ConversationSearchResult {
  matches: ConversationSearchMatch[];
  hasMore: boolean;
}
export interface GlobalConversationSearchMatch {
  threadId: string;
  threadTitle: string;
  workspaceId: string;
  workspaceLabel: string;
  workspacePath: string;
  turnId: string | null;
  itemId: string | null;
  kind: 'title' | 'message';
  role: string;
  text: string;
  createdAt: string;
}
export interface GlobalConversationSearchResult {
  matches: GlobalConversationSearchMatch[];
  hasMore: boolean;
  nextOffset: number | null;
  scope: 'device';
}
export function searchConversations(query: string, workspaceId: string | undefined, offset: number, signal?: AbortSignal) {
  const path = workspaceId ? `api/workspaces/${encodeURIComponent(workspaceId)}/search` : 'api/search';
  return request<GlobalConversationSearchResult>(
    `${path}?${new URLSearchParams({ q: query, limit: '50', offset: String(offset) })}`,
    { ...(signal ? { signal } : {}), cache: 'no-store' },
  );
}
export function searchThreadMessages(id: string, query: string, signal?: AbortSignal) {
  return request<ConversationSearchResult>(
    `api/threads/${encodeURIComponent(id)}/search?${new URLSearchParams({ q: query, limit: '50' })}`,
    { signal: signal ?? null },
  );
}
// Older Supervisors have no search endpoint. A full conversation page avoids
// the old summary + one detail request per turn; fetch only after a query.
export function fetchThreadConversationPage(id: string, beforeTurnId?: string, signal?: AbortSignal) {
  const params = new URLSearchParams({ view: 'full', limit: '100', compact: '1' });
  if (beforeTurnId) params.set('beforeTurnId', beforeTurnId);
  return request<ThreadDetailDto>(`api/threads/${encodeURIComponent(id)}?${params}`, { signal: signal ?? null });
}

export function fetchThreadHistoryItemDetail(id: string, itemId: string,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadHistoryItemDetailDto>(deviceId,
    `api/threads/${id}/items/${encodeURIComponent(itemId)}/detail`,
  );
}

export function fetchThreadTurnDetail(id: string, turnId: string,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadTurnDto>(deviceId,
    `api/threads/${id}/turns/${encodeURIComponent(turnId)}/detail?compact=1`,
  );
}

export function fetchPlugins() {
  return request<PluginDto[]>('api/plugins', {
    cache: 'no-store',
  });
}

export function importPlugin(input: ImportPluginInput) {
  return request<PluginDto>('api/plugins/import', {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function updatePlugin(pluginId: string, input: UpdatePluginInput) {
  return request<PluginDto>(`api/plugins/${encodeURIComponent(pluginId)}`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function deletePlugin(pluginId: string) {
  return request<PluginDto>(`api/plugins/${encodeURIComponent(pluginId)}`, {
    method: 'DELETE',
  });
}

export function fetchThreadExportTurns(id: string) {
  return request<ThreadExportTurnOptionsDto>(
    `api/threads/${id}/export-turns`,
    {
      cache: 'no-store',
    },
  );
}

export function fetchThreadShellState(id: string,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadShellStateDto>(deviceId, `api/threads/${id}/shell`);
}

export function createThread(input: CreateThreadInput) {
  return request<ThreadDto>('api/threads/start', {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function importThread(
  input: ImportThreadInput | ImportThreadInput['sessionId'],
) {
  const body = typeof input === 'string' ? { sessionId: input } : input;
  return request<ThreadDetailDto>('api/threads/import', {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

export function fetchImportThreadCandidates(
  provider: AgentBackendIdDto,
  agentId?: string | null,
) {
  const query = new URLSearchParams({ provider });
  if (agentId) {
    query.set('agentId', agentId);
  }
  return request<ImportThreadCandidateDto[]>(
    `api/threads/import-candidates?${query.toString()}`,
  );
}

export function createThreadShell(
  id: string,
  input: { cols?: number; rows?: number; label?: string } = {},
  deviceId?: string | null,
) {
  return deviceRequest<ThreadShellStateDto>(deviceId, `api/threads/${id}/shell`, {
    method: 'POST',
    ...(Object.keys(input).length > 0 ? { body: JSON.stringify(input) } : {}),
  });
}

export function terminateShell(id: string,
  deviceId?: string | null,
  threadScope?: string,
) {
  return deviceRequest<ShellSessionDto>(deviceId, `api/shells/${id}/terminate`, {
    method: 'POST',
  }, threadScope);
}

export function updateShell(id: string, input: UpdateShellInput,
  deviceId?: string | null,
  threadScope?: string,
) {
  return deviceRequest<ShellSessionDto>(deviceId, `api/shells/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  }, threadScope);
}

export function resumeThread(id: string, input: ResumeThreadInput = {},
  deviceId?: string | null,
) {
  return deviceRequest<ThreadDetailDto>(deviceId, `api/threads/${id}/resume`, {
    method: 'POST',
    ...(Object.keys(input).length > 0 ? { body: JSON.stringify(input) } : {}),
  });
}

export function disconnectThread(id: string) {
  return request<ThreadDetailDto>(`api/threads/${id}/disconnect`, {
    method: 'POST',
  });
}

export function sendThreadPrompt(
  id: string,
  input: SendThreadPromptRequestInput,
  deviceId?: string | null,
) {
  const attachments = input.attachments ?? [];

  if (attachments.length === 0) {
    return deviceRequest<ThreadDto>(deviceId, `api/threads/${id}/prompt`, {
      method: 'POST',
      body: JSON.stringify(input),
    });
  }

  const formData = new FormData();
  formData.append('prompt', input.prompt);
  if (input.clientRequestId !== undefined) {
    formData.append('clientRequestId', input.clientRequestId);
  }
  if (input.model !== undefined) {
    formData.append('model', input.model);
  }
  if (input.reasoningEffort !== undefined && input.reasoningEffort !== null) {
    formData.append('reasoningEffort', input.reasoningEffort);
  }
  if (input.collaborationMode !== undefined) {
    formData.append('collaborationMode', input.collaborationMode);
  }

  const manifest: Array<
    Pick<
      PromptAttachmentUpload,
      'clientId' | 'kind' | 'originalName' | 'placeholder'
    >
  > = attachments.map((attachment, index) => ({
    clientId: attachment.clientId,
    kind: attachment.kind,
    originalName: normalizedUploadFileName(attachment, index),
    placeholder: attachment.placeholder,
  }));
  formData.append('attachmentManifest', JSON.stringify(manifest));
  for (const [index, attachment] of attachments.entries()) {
    formData.append(
      'attachments',
      attachment.file,
      normalizedUploadFileName(attachment, index),
    );
  }

  return deviceRequest<ThreadDto>(deviceId, `api/threads/${id}/prompt`, {
    method: 'POST',
    body: formData,
  });
}

export function interruptThread(id: string, input: InterruptTurnInput = {},
  deviceId?: string | null,
) {
  return deviceRequest<ThreadDto>(deviceId, `api/threads/${id}/interrupt`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function updateThread(id: string, input: UpdateThreadInput) {
  return request<ThreadDto>(`api/threads/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function deleteThread(id: string) {
  return request<{ id: string }>(`api/threads/${id}`, {
    method: 'DELETE',
  });
}

export function cancelPendingSteer(id: string, pendingSteerId: string,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadDetailDto>(deviceId,
    `api/threads/${id}/pending-steers/${encodeURIComponent(pendingSteerId)}`,
    {
      method: 'DELETE',
    },
  );
}

export async function fetchThreadDelivery(id: string,
  deviceId?: string | null,
) {
  try {
    return await deviceRequest<ThreadDetailDto & {acceptedSteerIds?: string[]}>(deviceId, `api/threads/${id}?view=delivery`);
  } catch (error) {
    // Compatible with devices that have not installed the lightweight endpoint yet.
    if (error instanceof ApiError && error.statusCode === 400) return fetchThreadDetail(id, {limit:1}, deviceId);
    throw error;
  }
}

export async function steerPendingPrompt(id: string, pendingSteerId: string,
  deviceId?: string | null,
) {
  try {
    return await deviceRequest<ThreadDetailDto>(deviceId,
      `api/threads/${id}/pending-steers/${encodeURIComponent(pendingSteerId)}/steer`, {method:'POST'},
    );
  } catch (error) {
    if (!(error instanceof TypeError) && !(error instanceof ApiError && [408,502,503,504].includes(error.statusCode))) throw error;
    // A gateway timeout says nothing about delivery. Check the same durable ID;
    // never repeat the POST or treat a missing queue entry as proof of delivery.
    for (let attempt=0; attempt<4; attempt++) {
      if (attempt) await new Promise(resolve=>setTimeout(resolve, attempt*500));
      try {
        const delivery = await fetchThreadDelivery(id, deviceId);
        if ((delivery as ThreadDetailDto & {acceptedSteerIds?:string[]}).acceptedSteerIds?.includes(pendingSteerId)
          || delivery.pendingSteers.some(item=>item.id===pendingSteerId && item.delivery==='steer')
          || delivery.turns.some(turn=>turn.items.some(item=>item.id===`steer:${pendingSteerId}`))) return delivery;
      } catch { /* The connection can recover between confirmation reads. */ }
    }
    throw new Error(translate("workbench.steerDeliveryIsNotYetConfirmedCheck"));
  }
}

export async function steerSubmittedPrompt(
  id: string,
  clientRequestId: string,
  targetTurnId: string,
  deviceId?: string | null,
) {
  const delivery = await fetchThreadDelivery(id, deviceId);
  // If the original turn finished, the saved continuation starts normally.
  // Skip if it has already progressed to a new turn, and only use our receipt.
  if (delivery.thread.status !== 'running' || delivery.thread.activeTurnId !== targetTurnId) {
    return delivery;
  }
  const pending = delivery.pendingSteers.find(item => item.clientRequestId === clientRequestId);
  if (!pending) throw new Error(translate("workbench.theSavedMessageIsNotAvailableTo"));
  if (pending.delivery === 'steer') return delivery;
  return steerPendingPrompt(id, pending.id, deviceId);
}

export function updateThreadSettings(
  id: string,
  input: UpdateThreadSettingsInput,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadDto>(deviceId, `api/threads/${id}/settings`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function compactThread(id: string) {
  return request<ThreadDto>(`api/threads/${id}/compact`, {
    method: 'POST',
  });
}

export function fetchThreadGoal(id: string) {
  return request<{ goal: ThreadGoalDto | null }>(`api/threads/${id}/goal`, {
    cache: 'no-store',
  });
}

export function updateThreadGoal(id: string, input: UpdateThreadGoalInput) {
  return request<{ goal: ThreadGoalDto | null }>(`api/threads/${id}/goal`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export function clearThreadGoal(id: string) {
  return request<{ cleared: boolean; goalHistory?: ThreadGoalDto[] }>(
    `api/threads/${id}/goal`,
    {
      method: 'DELETE',
    },
  );
}

export function fetchThreadForkTurns(id: string) {
  return request<ThreadForkTurnOptionDto[]>(`api/threads/${id}/fork-turns`, {
    cache: 'no-store',
  });
}

export function forkThread(id: string, input: ForkThreadInput) {
  return request<ThreadForkResultDto>(`api/threads/${id}/fork`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function fetchThreadSkills(id: string) {
  return request<ThreadSkillsDto>(`api/threads/${id}/skills`, {
    cache: 'no-store',
  });
}

export function fetchThreadMcpServers(id: string) {
  return request<ThreadMcpServersDto>(`api/threads/${id}/mcp-servers`, {
    cache: 'no-store',
  });
}

export function fetchThreadHooks(id: string) {
  return request<ThreadHooksDto>(`api/threads/${id}/hooks`, {
    cache: 'no-store',
  });
}

export function createThreadHook(id: string, input: CreateThreadHookInput) {
  return request<ThreadHooksDto>(`api/threads/${id}/hooks`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function updateThreadHook(id: string, input: UpdateThreadHookInput) {
  return request<ThreadHooksDto>(`api/threads/${id}/hooks`, {
    method: 'PUT',
    body: JSON.stringify(input),
  });
}

export function trustThreadHook(id: string, input: TrustThreadHookInput) {
  return request<ThreadHooksDto>(`api/threads/${id}/hooks/trust`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function untrustThreadHook(id: string, input: UntrustThreadHookInput) {
  return request<ThreadHooksDto>(`api/threads/${id}/hooks/untrust`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function respondToThreadRequest(
  id: string,
  requestId: string,
  input: RespondThreadActionRequestInput,
  deviceId?: string | null,
) {
  return deviceRequest<ThreadDetailDto>(deviceId,
    `api/threads/${id}/requests/${encodeURIComponent(requestId)}/respond`,
    {
      method: 'POST',
      body: JSON.stringify(input),
    },
  );
}

export function createWorkspace(input: CreateWorkspaceInput) {
  return request<WorkspaceDto>('api/workspaces', {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function updateWorkspace(id: string, input: UpdateWorkspaceInput) {
  return request<WorkspaceDto>(`api/workspaces/${id}`, {
    method: 'PATCH',
    body: JSON.stringify(input),
  });
}

export type DeleteWorkspaceConfirmationInput = {
  confirmWorkspaceId: string;
  confirmLabel: string;
};

export function deleteWorkspace(
  id: string,
  input: DeleteWorkspaceConfirmationInput,
) {
  return request<{ id: string }>(`api/workspaces/${id}`, {
    method: 'DELETE',
    body: JSON.stringify(input),
  });
}

export function updateWorkspaceFavorite(
  id: string,
  input: UpdateWorkspaceFavoriteInput,
) {
  return request<WorkspaceDto>(`api/workspaces/${id}/favorite`, {
    method: 'POST',
    body: JSON.stringify(input),
  });
}

export function connectSupervisorEvents(
  onEvent: (event: ThreadEventEnvelope) => void,
  target?: { deviceId: string | null; threadId: string | null },
) {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = relayModeEnabled() ? encryptedRelaySocket(buildSocketUrl(protocol, target)) : new WebSocket(buildSocketUrl(protocol, target));

  socket.addEventListener('message', (message) => {
    try {
      const parsed = JSON.parse(
        message.data as string,
      ) as SupervisorSocketServerEnvelope;
      if (isThreadEventEnvelope(parsed)) {
        onEvent(parsed);
      }
    } catch {
      // Ignore malformed socket payloads.
    }
  });

  return socket;
}

export function connectShellSocket(
  handlers: {
    onConnected?: (event: SupervisorConnectedEnvelope) => void;
    onShellEvent?: (event: ShellEventEnvelope) => void;
  } = {},
  target?: { deviceId: string | null; threadId: string | null },
) {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = relayModeEnabled() ? encryptedRelaySocket(buildSocketUrl(protocol, target)) : new WebSocket(buildSocketUrl(protocol, target));

  socket.addEventListener('message', (message) => {
    try {
      const parsed = JSON.parse(
        message.data as string,
      ) as SupervisorSocketServerEnvelope;
      if (parsed.type === 'supervisor.connected') {
        handlers.onConnected?.(parsed);
        return;
      }
      if (isShellEventEnvelope(parsed)) {
        handlers.onShellEvent?.(parsed);
      }
    } catch {
      // Ignore malformed socket payloads.
    }
  });

  return {
    socket,
    send(message: SupervisorSocketClientEnvelope) {
      socket.send(JSON.stringify(message));
    },
  };
}

function buildSocketUrl(protocol: 'ws:' | 'wss:', target?: { deviceId: string | null; threadId: string | null }) {
  const url = new URL(appUrl('ws'), window.location.origin);
  url.protocol = protocol;
  if (relayModeEnabled()) {
    const deviceId = target ? target.deviceId : readSelectedRelayDeviceId();
    const relay = new URL(relaySocketUrl(deviceId
      ? `relay/devices/${encodeURIComponent(deviceId)}/ws`
      : 'relay/ws'));
    url.protocol = relay.protocol;
    url.hostname = relay.hostname;
    url.port = relay.port;
    url.pathname = relay.pathname;
  }
  const token = readStoredAuthToken();
  const relayToken = readStoredRelayToken();
  const relayThreadId = target ? target.threadId : readSelectedRelayThreadId();
  if (relayModeEnabled() && relayToken) {
    url.searchParams.set('relaySession', relayToken);
  }
  if (relayModeEnabled() && relayThreadId) {
    url.searchParams.set('threadId', relayThreadId);
  }
  if (token) {
    url.searchParams.set('token', token);
  }
  return url.toString();
}

function isThreadEventEnvelope(
  event: SupervisorSocketServerEnvelope,
): event is ThreadEventEnvelope {
  return (
    'threadId' in event &&
    event.type.startsWith('thread.') &&
    typeof event.payload === 'object' &&
    event.payload !== null
  );
}

function isShellEventEnvelope(
  event: SupervisorSocketServerEnvelope,
): event is ShellEventEnvelope {
  return (
    'shellId' in event &&
    event.type.startsWith('shell.') &&
    typeof event.payload === 'object' &&
    event.payload !== null
  );
}

export function workspaceResourceScope(deviceId?: string | null) {
  return JSON.stringify([window.location.origin, deviceId === undefined ? readSelectedRelayDeviceId() : deviceId, workspaceActorScope]);
}
export function fetchWorkspaceDocument(workspaceId: string, path: string, signal?: AbortSignal,
  deviceId?: string | null,
) {
  return deviceRequest<WorkspaceDocumentSnapshot>(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files/document?${new URLSearchParams({path})}`, { cache: 'no-store', ...(signal ? {signal} : {}) });
}
export async function saveWorkspaceDocument(workspaceId: string, input: WorkspaceDocumentSaveInput,
  deviceId?: string | null,
): Promise<WorkspaceSaveReceipt> {
  try { return await deviceRequest<WorkspaceSaveReceipt>(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files/save`, {method:'POST', body: JSON.stringify(input)}); }
  catch (error) {
    if (error instanceof ApiError && error.payload.details && ['conflict','failedBeforeWrite'].includes(String(error.payload.details.status))) return error.payload.details as unknown as WorkspaceSaveReceipt;
    if (error instanceof ApiError && [400,401,403,404,409,410,413,415,422,501,507].includes(error.statusCode)) return {status:'failedBeforeWrite',operationId:input.operationId,draftRevision:input.draftRevision,path:input.path,code:error.statusCode===401||error.statusCode===403?'forbidden':error.payload.code,message:error.payload.message};
    throw error;
  }
}
export function fetchWorkspaceSaveOperation(workspaceId: string, operationId: string,
  deviceId?: string | null,
) {
  return deviceRequest<WorkspaceSaveReceipt>(deviceId, `api/workspaces/${encodeURIComponent(workspaceId)}/files/operations/${encodeURIComponent(operationId)}`, { cache:'no-store' });
}
