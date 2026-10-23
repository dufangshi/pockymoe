import type { RelayRegistrationSettingsDto } from '@pockymoe/shared';
// Calls of the relay admin console. Every request uses the admin realm
// (bearer token or admin cookie), never the ordinary relay session.
import { request } from '../../lib/api';
import type {
  AdminDevices,
  AdminDeviceDetail,
  AdminOverview,
  AdminPreviews,
  AdminPublicLinks,
  AdminRange,
  AdminRegistrations,
  AdminRetention,
  AdminSearchResult,
  AdminSecuritySummary,
  AdminSecurityEvent,
  AdminSession,
  AdminShares,
  AdminSystem,
  AdminUserDetail,
  AdminUsers,
  AdminAction,
  AdminPerformance,
  AdminPerformanceDevices,
  AdminReconnectRow,
  AdminServer,
  AdminTrafficRow,
  Paged,
} from './types';

/** A chart window: a preset range or a custom `from`/`to` (epoch ms). */
export type WindowParams = { range?: AdminRange; from?: number; to?: number };

type Params = Record<string, string | number | null | undefined>;
/** A list request: paging, search, sort and column filters. */
type ListParams = Params;

function query(params: Params) {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '' && value !== 'all') search.set(key, String(value));
  }
  const text = search.toString();
  return text ? `?${text}` : '';
}

function admin<T>(path: string, init?: RequestInit) {
  return request<T>(`relay/admin/${path}`, init, { auth: 'relay-admin' });
}

const id = encodeURIComponent;

export const adminApi = {
  overview: (window: WindowParams) => admin<AdminOverview>(`overview${query(window)}`),
  search: (q: string) => admin<AdminSearchResult>(`search${query({ q })}`),
  users: (params: ListParams) =>
    admin<AdminUsers>(`users${query(params)}`),
  user: (userId: string) => admin<AdminUserDetail>(`users/${id(userId)}`),
  endUserSessions: (userId: string) =>
    admin<{ id: string; ended: number }>(`users/${id(userId)}/sessions`, { method: 'DELETE' }),
  registrations: (params: { status?: 'pending' | 'reviewed'; page?: number; pageSize?: number; sort?: string | undefined; dir?: string | undefined }) =>
    admin<AdminRegistrations>(`registrations${query(params)}`),
  devices: (params: ListParams) =>
    admin<AdminDevices>(`devices${query(params)}`),
  device: (deviceId: string, range?: AdminRange) => admin<AdminDeviceDetail>(`devices/${id(deviceId)}${query({ range: range === '24h' ? undefined : range })}`),
  disconnectDevice: (deviceId: string) =>
    admin<{ id: string; wasConnected: boolean }>(`devices/${id(deviceId)}/disconnect`, { method: 'POST' }),
  shares: (params: ListParams) =>
    admin<AdminShares>(`shares${query(params)}`),
  revokeShare: (kind: 'share' | 'grant', shareId: string) =>
    admin<{ id: string; revokedAt: string }>(`shares/${id(kind)}/${id(shareId)}`, { method: 'DELETE' }),
  publicLinks: (params: ListParams) =>
    admin<AdminPublicLinks>(`public-links${query(params)}`),
  previews: () => admin<AdminPreviews>('previews'),
  security: (window: WindowParams) => admin<AdminSecuritySummary>(`security${query(window)}`),
  securityEvents: (params: ListParams) =>
    admin<Paged<AdminSecurityEvent>>(`security/events${query(params)}`),
  sessions: (params: ListParams) =>
    admin<Paged<AdminSession>>(`sessions${query(params)}`),
  endSession: (sessionId: string) => admin<{ id: string }>(`sessions/${id(sessionId)}`, { method: 'DELETE' }),
  actions: (params: ListParams) =>
    admin<Paged<AdminAction>>(`actions${query(params)}`),
  performance: (params: WindowParams) =>
    admin<AdminPerformance>(`performance${query(params)}`),
  reconnectDevices: (params: WindowParams & ListParams) =>
    admin<AdminPerformanceDevices<AdminReconnectRow>>(`performance/devices${query({ ...params, view: 'reconnects' })}`),
  trafficDevices: (params: WindowParams & ListParams) =>
    admin<AdminPerformanceDevices<AdminTrafficRow>>(`performance/devices${query({ ...params, view: 'traffic' })}`),
  system: (window: WindowParams = {}) => admin<AdminSystem>(`system${query(window)}`),
  server: (window: WindowParams) => admin<AdminServer>(`server${query(window)}`),
  registrationSettings: () =>
    admin<{ registrationEnabled: boolean; settings: RelayRegistrationSettingsDto }>('settings/registration'),
  retention: () => admin<AdminRetention>('settings/retention'),
  updateRetention: (input: Partial<Pick<AdminRetention, 'metricsDays' | 'securityDays'>>) =>
    admin<AdminRetention>('settings/retention', { method: 'PATCH', body: JSON.stringify(input) }),
};
