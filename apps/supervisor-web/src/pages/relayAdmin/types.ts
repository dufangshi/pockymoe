// Shapes of the relay admin API (crates/relay/src/admin, docs/relay-admin.md).
// Fields that newer relays add are optional so an older relay still renders.

export type AdminRange = '24h' | '7d' | '30d' | '90d';

export interface Paged<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
}

/** A metric over time: `points` are [epoch ms, value or null for a gap]. */
export interface AdminSeries {
  resolutionSeconds: number;
  points: Array<[number, number | null]>;
  /** When recording started on this relay; earlier points are not zeros but unknown. */
  historyStart: string | null;
}

export interface AdminSecurityEvent {
  id: number;
  at: string;
  event: string;
  category: 'login' | 'challenge' | 'reauth' | 'failure' | 'activity' | 'credential';
  userId: string | null;
  username: string | null;
  resourceId: string | null;
  resourceLabel: string | null;
}

export interface AdminAction {
  id: number;
  at: string;
  adminUsername: string;
  action: string;
  targetType: string | null;
  targetId: string | null;
  targetLabel: string | null;
  detail: Record<string, unknown> | null;
}

export interface AdminRegistrationSummary {
  pending: number;
  oldestPendingAt: string | null;
  approved30d: number;
  rejected30d: number;
  medianApprovalHours: number | null;
}

export interface AdminPublicLinkSummary {
  total: number;
  encrypted: number;
  legacy: number;
  live: number;
  bytes: number;
}

export interface AdminVersionCount {
  version: string | null;
  devices: number;
  online: number;
}

export interface AdminOverview {
  generatedAt: string;
  range: AdminRange;
  rangeStart: string;
  relay: { version: string; startedAt: string; publicBaseUrl: string | null };
  attention: {
    pendingRegistrations: number;
    oldestPendingAt: string | null;
    usersWithoutAccountKey: number;
    legacyPublicLinks: number;
    /** Null until the relay records tunnel connections. */
    unstableDevices: number | null;
  };
  users: {
    total: number;
    enabled: number;
    disabled: number;
    active1h: number;
    active24h: number;
    active7d: number;
    active30d: number;
    neverSeen: number;
    newInRange: number;
    withAccountKey: number;
    twoFactor: number;
    resetPending: number;
    /** Distinct people active per day, when the relay records it. */
    dailyActive?: Array<{ day: string; users: number; devices: number }> | null;
  };
  activity: Array<{ key: '1h' | '24h' | '7d' | '30d' | 'older' | 'never'; count: number }>;
  devices: {
    total: number;
    online: number;
    protected: number;
    unprotected: number;
    newThisWeek: number;
    newInRange: number;
  };
  shares: { active: number; createdThisWeek: number; revokedThisWeek: number; expiringSoon: number };
  registrations: AdminRegistrationSummary;
  signupsWeekly: Array<{ weekStart: string; count: number }>;
  publicLinks: AdminPublicLinkSummary;
  securityEvents: AdminSecurityEvent[];
  onlineDevices?: AdminSeries | null;
  versions?: AdminVersionCount[] | null;
  tunnels?: AdminTunnelSummary | null;
  adminActions?: AdminAction[] | null;
}

export interface AdminTunnelSummary {
  reconnects: number;
  unstableDevices: number;
  requests: number;
  errorRate: number | null;
  p95Ms: number | null;
  reconnectSeries: AdminSeries;
  errorSeries: AdminSeries;
  latencySeries: AdminSeries;
}

export interface AdminUser {
  id: string;
  username: string;
  email: string;
  role: 'admin' | 'user';
  enabled: boolean;
  createdAt: string;
  lastSeenAt: string | null;
  loginMethods: string[];
  oauthOnly: boolean;
  /** Null for the admin, which has no account key. */
  accountKey: boolean | null;
  resetPending: boolean;
  authenticator: boolean;
  passkeys: number;
  devices: number;
  devicesOnline: number;
}

export type AdminUserFilter = 'all' | 'active7d' | 'noAccountKey' | 'noTwoFactor' | 'disabled' | 'neverSeen';

export interface AdminUsers extends Paged<AdminUser> {
  counts: Record<AdminUserFilter, number>;
  methods: {
    users: number;
    password: number;
    google: number;
    github: number;
    oauthOnly: number;
    authenticator: number;
    passkey: number;
    twoFactor: number;
    accountKey: number;
  };
  pendingRegistrations: number;
}

export interface AdminSession {
  id: string;
  environment: string | null;
  createdAt: string;
  expiresAt: string;
  strongAuthAt: string | null;
  trustedBrowser: boolean;
  accountMode: string;
  userId?: string;
  username?: string;
  role?: string;
}

export interface AdminDevice {
  id: string;
  name: string;
  ownerUserId: string;
  ownerUsername: string;
  createdAt: string;
  connected: boolean;
  connectedAt: string | null;
  lastHeartbeatAt: string | null;
  protected: boolean;
  protectedAt: string | null;
  threadCount: number;
  agentThreadCount: number;
  workspaceCount: number;
  portMappings: number;
  /** Recorded by newer relays. */
  version?: string | null;
  versionCheckedAt?: string | null;
  outdated?: boolean | null;
  lastSeenAt?: string | null;
  reconnects1h?: number | null;
  unstable?: boolean | null;
  /** Forwarded requests per day for the last 7 days, oldest first. */
  traffic7d?: number[] | null;
}

export type AdminDeviceFilter = 'all' | 'online' | 'offline' | 'unstable' | 'unprotected' | 'outdated';

export interface AdminDevices extends Paged<AdminDevice> {
  counts: Partial<Record<AdminDeviceFilter | 'protected', number>> & { all: number };
  relayVersion: string;
}

export interface AdminConnectionSpan {
  from: string;
  to: string | null;
  reason: string | null;
}

export interface AdminDeviceDetail {
  device: AdminDevice;
  upstreamProtocols: string[];
  shares: Array<{ id: string; kind: 'share' | 'grant'; scope: 'thread' | 'workspace' | 'device'; targetUsername: string | null }>;
  previews: { mappings: number; sessions: number; streams: number };
  connections?: {
    since: string;
    historyStart: string | null;
    spans: AdminConnectionSpan[];
    uptime: number | null;
    reconnects: number;
    longestOfflineSeconds: number | null;
  } | null;
  traffic?: {
    resolutionSeconds: number;
    historyStart: string | null;
    hours: Array<{ at: string; requests: number; requestBytes: number; responseBytes: number; errors: number }>;
  } | null;
}

export interface AdminUserDetail {
  user: AdminUser;
  devices: AdminDevice[];
  sessions: AdminSession[];
  shares: { outgoing: number; incoming: number };
  accountBrowsers: number;
  trustedBrowsers: number;
  events: AdminSecurityEvent[];
  registration: { provider: string; requestedAt: string; approvedAt: string | null; approvedBy: string | null } | null;
}

export interface AdminRegistration {
  id: string;
  username: string;
  email: string;
  provider: string;
  createdAt: string;
  status: 'pending' | 'approved' | 'rejected';
  reviewedAt: string | null;
  reviewedBy: string | null;
}

export interface AdminRegistrations extends Paged<AdminRegistration> {
  summary: AdminRegistrationSummary;
}

export type AdminShareScope = 'thread' | 'workspace' | 'device';
export type AdminShareFilter = 'all' | AdminShareScope | 'expiring' | 'unused';

export interface AdminShare {
  id: string;
  kind: 'share' | 'grant';
  scope: AdminShareScope;
  ownerUserId: string;
  ownerUsername: string;
  targetUserId: string;
  targetUsername: string;
  deviceId: string;
  deviceName: string;
  workspaceLabel: string | null;
  threadAccess: 'read' | 'control';
  workspaceAccess: 'none' | 'read' | 'write';
  canCreateThreads: boolean;
  createdAt: string;
  expiresAt: string | null;
  lastAccessedAt: string | null;
  lastAccessKind: string | null;
}

export interface AdminShares extends Paged<AdminShare> {
  counts: Record<AdminShareFilter, number>;
  summary: {
    active: number;
    createdThisWeek: number;
    revokedThisWeek: number;
    expiringSoon: number;
    byScope: Record<AdminShareScope, number>;
    permissions: { control: number; read: number; filesWrite: number; canCreateThreads: number };
  };
  usage: { days: Array<{ day: string; kinds: Record<string, number> }>; approximate: boolean };
}

export interface AdminPublicLink {
  id: string;
  ownerUsername: string;
  deviceName: string;
  createdAt: string;
  refreshedAt: string | null;
  live: boolean;
  encrypted: boolean;
  bytes: number;
}

export interface AdminPublicLinks extends Paged<AdminPublicLink> {
  summary: AdminPublicLinkSummary;
}

export interface AdminPreviews {
  configured: boolean;
  mappings: number;
  sessions: number;
  streams: number;
  devices: Array<{ deviceId: string; deviceName: string; ownerUsername: string | null; mappings: number; sessions: number; streams: number }>;
}

export interface AdminSecuritySummary {
  range: AdminRange;
  logins: number;
  mfaLogins: number;
  activeSessions: number;
  trustedBrowsers: number;
  factors: AdminUsers['methods'];
  daily: Array<{ day: string; categories: Partial<Record<AdminSecurityEvent['category'], number>> }>;
  environments: Array<{ label: string | null; count: number }>;
  history: { oldestEventAt: string | null; events: number };
  /** Counted by newer relays. */
  loginFailures?: { total: number; daily: Array<{ day: string; count: number }>; historyStart: string | null } | null;
  rateLimited?: { total: number; byKind: Record<string, number>; daily: Array<{ day: string; count: number }>; historyStart: string | null } | null;
  retention?: { securityDays: number; securityMaxEvents: number } | null;
  /** Admin actions in the range; present when the relay keeps the admin action log. */
  adminActions?: number | null;
}

export interface AdminSystemCheck {
  key: string;
  ok: boolean;
  detail: string | number | null;
}

export interface AdminSystem {
  relay: {
    version: string;
    startedAt: string;
    uptimeSeconds: number;
    publicBaseUrl: string | null;
    webOrigin: string | null;
    servesWeb: boolean;
    dataDir: string | null;
  };
  connections: {
    deviceTunnels: number;
    browserSockets: number;
    pendingRequests: number;
    previewMappings: number;
    previewSessions: number;
    previewStreams: number;
  };
  checks: AdminSystemCheck[];
  push: {
    subscriptions: { webPush: number; apns: number };
    queued: number;
    retrying: number;
    turns24h: Record<string, number>;
    apnsConfigured: boolean;
    /** Delivery results of the last 24 hours, counted by newer relays. */
    delivery?: { sent: number; failed: number; expired: number; historyStart: string | null } | null;
  };
  storage: {
    databaseBytes: number;
    walBytes: number;
    tables: Array<{ name: string; bytes: number | null; rows: number | null }>;
    measuredAt: string;
    /** Database size over 30 days, recorded by newer relays. */
    growth?: AdminSeries | null;
  };
  versions?: {
    current: AdminVersionCount[];
    /** Online devices per version per day for the top versions. */
    history: Array<{ day: string; versions: Record<string, number> }>;
    historyStart: string | null;
  } | null;
  metrics?: { retentionDays: number; historyStart: string | null } | null;
}

export interface AdminSearchResult {
  users: Array<{ id: string; username: string; email: string; role: string; enabled: boolean }>;
  devices: Array<{ id: string; name: string; ownerUsername: string }>;
}

export interface AdminRetention {
  metricsDays: number;
  securityDays: number;
  options: { metricsDays: number[]; securityDays: number[] };
  fixed: { connectionDays: number; adminActionDays: number; securityMaxEvents: number };
}
