import { translate, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import type { UpstreamConnectionProtocol } from '@pockymoe/thread-ui';
import { ApiError, request } from './api';

export type ConnectionProtocol = UpstreamConnectionProtocol;
export const connectionProtocols: ConnectionProtocol[] = [
  'openai_chat',
  'openai_responses',
  'anthropic',
  'gemini',
];

export type ConnectionSource = {
  deviceId: string;
  name: string;
  online: boolean;
  revision: string;
};

export type ConnectionSummary = {
  id: string;
  name: string;
  protocol: ConnectionProtocol;
  baseUrl?: string;
  authType?: 'api_key' | 'bearer';
  model?: string;
  headers?: Record<string, string>;
  hasApiKey?: boolean;
  revision: string;
  local: boolean;
  sources: ConnectionSource[];
  conflict?: boolean;
  legacyHarness?: string;
  /** What the upstream declared when the Supervisor last inspected it. */
  catalog?: UpstreamCatalog;
};

export type UpstreamCatalog = {
  checkedAt: string;
  /** Wire formats the endpoint serves with this credential. */
  protocols: ConnectionProtocol[];
  models: DiscoveredModel[];
  truncated?: boolean;
};

export type HarnessBinding = {
  connectionId?: string | null;
  profileId?: string | null;
  model?: string | null;
  revision?: string | null;
};

export type ConnectionCatalog = {
  connections: ConnectionSummary[];
  bindings: Record<string, HarnessBinding>;
  deviceId: string | null;
  catalogAvailable: boolean;
  catalogError?: string;
  /** Wire formats each managed harness adapter accepts. */
  harnessProtocols?: Record<string, ConnectionProtocol[]>;
  /**
   * Each harness's own sign-in or native configuration on this device. It belongs to
   * that harness only: never a saved connection, never synced or offered to plugins.
   */
  native?: Record<string, NativeUpstream>;
};

export type NativeUpstream = {
  available: boolean;
  active: boolean;
  kind: 'oauth' | 'config';
  revision?: string;
};

export type ConnectionDraft = {
  id?: string;
  name: string;
  protocol: ConnectionProtocol;
  baseUrl: string;
  apiKey?: string;
  authType?: 'api_key' | 'bearer';
  model?: string;
  headers?: Record<string, string>;
  revision?: string;
};

export type DiscoveredModel = {
  id: string;
  name?: string;
  aliases?: string[];
  contextWindow?: number;
  maxOutputTokens?: number;
  input?: string[];
};

type Discovery = {
  models: DiscoveredModel[];
  truncated?: boolean;
  protocols?: ConnectionProtocol[];
  checkedAt?: string;
};

/** `apiRoot` is `api` or `relay/devices/{id}/api`, matching the other management pages. */
export function connectionsApi(apiRoot: string) {
  const base = `${apiRoot}/management/connections`;
  const post = <T>(path: string, body: unknown, signal?: AbortSignal) =>
    request<T>(`${base}${path}`, {
      method: 'POST',
      body: JSON.stringify(body),
      ...(signal ? { signal } : {}),
    });
  return {
    async list(signal?: AbortSignal) {
      const value = await request<ConnectionCatalog>(base, {
        cache: 'no-store',
        ...(signal ? { signal } : {}),
      }).catch((error: unknown) => {
        // Supervisors released before saved connections do not have this API.
        if (error instanceof ApiError && [404, 405].includes(error.statusCode))
          throw Error(translate('settings.connUpdateSupervisor'));
        throw error;
      });
      if (!Array.isArray(value?.connections))
        throw Error(translate('settings.connUpdateSupervisor'));
      return {
        connections: value.connections,
        bindings: value.bindings ?? {},
        deviceId: value.deviceId ?? null,
        catalogAvailable: !!value.catalogAvailable,
        ...(value.catalogError ? { catalogError: value.catalogError } : {}),
        harnessProtocols: { ...harnessProtocols, ...value.harnessProtocols },
        // Older Supervisors report no native state: nothing is assumed.
        ...(value.native && typeof value.native === 'object'
          ? { native: value.native }
          : {}),
      } satisfies ConnectionCatalog;
    },
    save: (draft: ConnectionDraft) => post<ConnectionSummary>('', draft),
    remove: (id: string) =>
      request<unknown>(`${base}/${encodeURIComponent(id)}`, {
        method: 'DELETE',
      }),
    /** Saved connections reuse their stored key; drafts send their fields. */
    models: (target: { id: string } | ConnectionDraft, signal?: AbortSignal) =>
      'name' in target
        ? post<Discovery>('/models', target, signal)
        : post<Discovery>(
            `/${encodeURIComponent(target.id)}/models`,
            {},
            signal,
          ),
    test: (target: { id: string; model?: string } | ConnectionDraft) =>
      'name' in target
        ? post<{ ok: boolean; latencyMs?: number }>('/test', target)
        : post<{ ok: boolean; latencyMs?: number }>(
            `/${encodeURIComponent(target.id)}/test`,
            target.model ? { model: target.model } : {},
          ),
    /** Switches a harness back to its own sign-in or native configuration. */
    activateNative: (harness: string, revision?: string) =>
      request<{ ok?: boolean }>(
        `${apiRoot}/management/upstreams/native/${encodeURIComponent(harness)}/activate`,
        {
          method: 'POST',
          body: JSON.stringify(revision ? { revision } : {}),
        },
      ),
    apply: (id: string, harness: string, model?: string) =>
      post<{ ok: boolean; profileId?: string }>(
        `/${encodeURIComponent(id)}/apply`,
        model ? { harness, model } : { harness },
      ),
  };
}

/**
 * Imports a remote connection's credentials onto the current device only. The source
 * authorizes the exact target and revision first; nothing is applied to a harness.
 */
export async function syncConnection(
  apiRoot: string,
  connection: ConnectionSummary,
  source: ConnectionSource,
  currentDeviceId: string,
) {
  const id = encodeURIComponent(connection.id);
  const authorization = await request<{ token: string; expiresAt?: string }>(
    `relay/devices/${encodeURIComponent(source.deviceId)}/api/management/connections/${id}/transfer-authorizations`,
    {
      method: 'POST',
      body: JSON.stringify({
        targetDeviceId: currentDeviceId,
        revision: source.revision,
      }),
    },
  ).catch((error: unknown) => {
    throw new SyncStepError('authorize', error);
  });
  return request<ConnectionSummary>(
    `${apiRoot}/management/connections/${id}/sync`,
    {
      method: 'POST',
      body: JSON.stringify({
        sourceDeviceId: source.deviceId,
        revision: source.revision,
        token: authorization.token,
        ...(connection.local ? { expectedRevision: connection.revision } : {}),
      }),
    },
  ).catch((error: unknown) => {
    throw new SyncStepError('import', error);
  });
}

export class SyncStepError extends Error {
  constructor(
    readonly step: 'authorize' | 'import',
    readonly cause: unknown,
  ) {
    super(cause instanceof Error ? cause.message : String(cause));
  }
}

export type ConnectionState = 'local' | 'remote' | 'offline' | 'conflict';

export function remoteSources(
  connection: ConnectionSummary,
  deviceId: string | null,
) {
  return connection.sources.filter((s) => s.deviceId !== deviceId);
}

/** Online sources that hold a revision this device does not have yet. */
export function syncSources(
  connection: ConnectionSummary,
  deviceId: string | null,
) {
  if (!deviceId) return [];
  const seen = new Set<string>();
  return remoteSources(connection, deviceId).filter((s) => {
    if (!s.online) return false;
    if (connection.local && s.revision === connection.revision) return false;
    if (seen.has(s.revision)) return false;
    seen.add(s.revision);
    return true;
  });
}

export function connectionState(
  connection: ConnectionSummary,
  deviceId: string | null,
): ConnectionState {
  const remote = remoteSources(connection, deviceId);
  if (
    connection.conflict ||
    (connection.local && remote.some((s) => s.revision !== connection.revision))
  )
    return 'conflict';
  if (connection.local) return 'local';
  return remote.some((s) => s.online) ? 'remote' : 'offline';
}

/** Fallback for Supervisors whose catalog omits `harnessProtocols`. */
export const harnessProtocols: Record<string, ConnectionProtocol[]> = {
  codex: ['openai_responses'],
  claude: ['anthropic'],
  gemini: ['gemini'],
  grok: ['openai_responses', 'openai_chat'],
  deepseek: ['anthropic', 'openai_chat', 'openai_responses'],
};

/** The saved format first, then the others the upstream was seen to serve. */
export function servedProtocols(c: ConnectionSummary): ConnectionProtocol[] {
  return [...new Set([c.protocol, ...(c.catalog?.protocols ?? [])])];
}

/** The window the upstream declares for a model, if it does. */
export function declaredContextWindow(c: ConnectionSummary, model?: string) {
  const id = model || c.model;
  return c.catalog?.models.find(
    (m) => m.id === id || m.aliases?.includes(id ?? ''),
  )?.contextWindow;
}

/** 200192 → "200K", 1048576 → "1M". */
export function tokenCount(tokens: number) {
  if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M`;
  return `${Math.round(tokens / 1000)}K`;
}

/** Compact protocol tag for dense rows; the full label stays in forms and hints. */
export function protocolShort(protocol: string) {
  return (
    {
      openai_chat: 'Chat',
      openai_responses: 'Responses',
      anthropic: 'Anthropic',
      gemini: 'Gemini',
    }[protocol] ?? protocol
  );
}

export function protocolLabel(protocol: string) {
  switch (protocol) {
    case 'openai_chat':
      return translate('settings.connProtocolOpenAIChat');
    case 'openai_responses':
      return translate('settings.connProtocolOpenAIResponses');
    case 'anthropic':
      return translate('settings.connProtocolAnthropic');
    case 'gemini':
      return translate('settings.connProtocolGemini');
    default:
      return protocol;
  }
}

const errorKeys: Record<string, TranslationKey> = {
  peer_access_disabled: 'settings.connErrorPeerAccess',
  device_not_certified: 'settings.connErrorNotCertified',
  source_offline: 'settings.connErrorSourceOffline',
  sync_conflict: 'settings.connErrorSyncConflict',
  revision_mismatch: 'settings.connErrorRevisionMoved',
  transfer_token_invalid: 'settings.connErrorTokenExpired',
  transfer_token_expired: 'settings.connErrorTokenExpired',
  edit_conflict: 'settings.connErrorEditConflict',
  connection_in_use: 'settings.connErrorInUse',
  harness_busy: 'settings.connErrorHarnessBusy',
  protocol_unsupported: 'settings.connErrorProtocol',
};

export function errorCode(error: unknown) {
  const cause = error instanceof SyncStepError ? error.cause : error;
  return cause instanceof ApiError
    ? (cause.payload as { code?: string })?.code
    : undefined;
}

/** A short actionable message; unknown failures keep the server's own text. */
export function connectionErrorMessage(error: unknown, fallback: string) {
  const cause = error instanceof SyncStepError ? error.cause : error;
  const code = errorCode(error);
  if (code && errorKeys[code]) return translate(errorKeys[code]);
  if (cause instanceof ApiError) {
    if (
      error instanceof SyncStepError &&
      error.step === 'authorize' &&
      [502, 503, 504].includes(cause.statusCode)
    )
      return translate('settings.connErrorSourceOffline');
    if (cause.statusCode === 409 && !code)
      return translate('settings.connErrorChanged');
  }
  return cause instanceof Error && cause.message ? cause.message : fallback;
}
