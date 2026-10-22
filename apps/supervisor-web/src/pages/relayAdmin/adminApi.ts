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
  Paged,
} from './types';

type Params = Record<string, string | number | null | undefined>;

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
  overview: (range: AdminRange) => admin<AdminOverview>(`overview${query({ range })}`),
  search: (q: string) => admin<AdminSearchResult>(`search${query({ q })}`),
  users: (params: { filter?: string; q?: string; page?: number; pageSize?: number; sort?: string }) =>
    admin<AdminUsers>(`users${query(params)}`),
  user: (userId: string) => admin<AdminUserDetail>(`users/${id(userId)}`),
  endUserSessions: (userId: string) =>
    admin<{ id: string; ended: number }>(`users/${id(userId)}/sessions`, { method: 'DELETE' }),
  registrations: (params: { status?: 'pending' | 'reviewed'; page?: number; pageSize?: number }) =>
    admin<AdminRegistrations>(`registrations${query(params)}`),
  devices: (params: { filter?: string; q?: string; page?: number; pageSize?: number; sort?: string }) =>
    admin<AdminDevices>(`devices${query(params)}`),
  device: (deviceId: string) => admin<AdminDeviceDetail>(`devices/${id(deviceId)}`),
  disconnectDevice: (deviceId: string) =>
    admin<{ id: string; wasConnected: boolean }>(`devices/${id(deviceId)}/disconnect`, { method: 'POST' }),
  shares: (params: { filter?: string; q?: string; page?: number; pageSize?: number }) =>
    admin<AdminShares>(`shares${query(params)}`),
  revokeShare: (kind: 'share' | 'grant', shareId: string) =>
    admin<{ id: string; revokedAt: string }>(`shares/${id(kind)}/${id(shareId)}`, { method: 'DELETE' }),
  publicLinks: (params: { filter?: string; page?: number; pageSize?: number }) =>
    admin<AdminPublicLinks>(`public-links${query(params)}`),
  previews: () => admin<AdminPreviews>('previews'),
  security: (range: AdminRange) => admin<AdminSecuritySummary>(`security${query({ range })}`),
  securityEvents: (params: { type?: string; user?: string; range?: AdminRange; page?: number; pageSize?: number }) =>
    admin<Paged<AdminSecurityEvent>>(`security/events${query(params)}`),
  sessions: (params: { q?: string; page?: number; pageSize?: number }) =>
    admin<Paged<AdminSession>>(`sessions${query(params)}`),
  endSession: (sessionId: string) => admin<{ id: string }>(`sessions/${id(sessionId)}`, { method: 'DELETE' }),
  actions: (params: { page?: number; pageSize?: number; type?: string }) =>
    admin<Paged<AdminAction>>(`actions${query(params)}`),
  system: () => admin<AdminSystem>('system'),
  registrationSettings: () =>
    admin<{ registrationEnabled: boolean; settings: RelayRegistrationSettingsDto }>('settings/registration'),
  retention: () => admin<AdminRetention>('settings/retention'),
  updateRetention: (input: Partial<Pick<AdminRetention, 'metricsDays' | 'securityDays'>>) =>
    admin<AdminRetention>('settings/retention', { method: 'PATCH', body: JSON.stringify(input) }),
};
