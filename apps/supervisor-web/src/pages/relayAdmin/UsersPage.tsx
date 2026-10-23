// Users: the people on this relay (filtered, paged, with details and the
// admin actions on them), registration requests, and how people sign in.
import { confirmAsync, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Ban,
  Check,
  Fingerprint,
  History,
  KeyRound,
  LogOut,
  MonitorSmartphone,
  ShieldCheck,
  Trash2,
  UserCheck,
  UserPlus,
  Users,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { FormDialog } from '../../components/FormDialog';
import {
  approveRelayRegistration,
  deleteRelayAdminUser,
  rejectRelayRegistration,
  resetRelayAdminUserPassword,
  setRelayUserEnabled,
} from '../../lib/api';
import { adminApi } from './adminApi';
import { BarList, ChartCard } from './charts';
import { formatCount, formatDate, formatDateTime, formatDuration, formatRelative } from './format';
import { loginMethodLabel } from './labels';
import { EventList } from './events';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminRegistration, AdminSession, AdminUser, AdminUserDetail, AdminUserFilter } from './types';
import { FilterBar, SelectFilter, TimeFilter, windowParams } from './filters';
import { AdminTable, useFilterValues, useTableSettings, useUrlSort, type TableColumn } from './table';
import {
  Avatar,
  Badge,
  Card,
  DetailSection,
  DetailWindow,
  DeviceStateMark,
  EmptyState,
  errorText,
  LoadFailure,
  MiniStat,
  Pagination,
  ProtectionMark,
  RowMenu,
  RowOpener,
  SearchField,
  Skeleton,
  SubTabs,
  useAdminResource,
  useDebounced,
  type MenuItem,
} from './ui';

const PAGE_SIZE = 25;
const FILTER_KEYS = ['role', 'status', 'key', 'factor', 'method', 'seen', 'created'] as const;
type FilterKey = (typeof FILTER_KEYS)[number];
type UsersTab = 'users' | 'registrations' | 'methods';

export function UsersPage(props: AdminPageProps) {
  useI18n();
  const tab: UsersTab = props.sub === 'registrations' || props.sub === 'methods' ? props.sub : 'users';
  return (
    <div className="admin-page">
      <SubTabs
        idPrefix="admin-users"
        label={translate('admin.navUsers')}
        onChange={(key) => props.go('users', { sub: key === 'users' ? null : key })}
        tabs={[
          { key: 'users', label: translate('admin.navUsers'), icon: <Users aria-hidden className="h-4 w-4" /> },
          { key: 'registrations', label: translate('admin.registrations'), icon: <UserPlus aria-hidden className="h-4 w-4" /> },
          { key: 'methods', label: translate('admin.loginMethods'), icon: <Fingerprint aria-hidden className="h-4 w-4" /> },
        ]}
        value={tab}
      />
      <div aria-labelledby={`admin-users-tab-${tab}`} className="admin-tabpanel" id="admin-users-panel" role="tabpanel">
        {tab === 'users' ? <UserList {...props} /> : tab === 'registrations' ? <Registrations {...props} /> : <Methods {...props} />}
      </div>
    </div>
  );
}

/** Links from before the column filters (`filter=noAccountKey`) still work. */
function legacyFilter(filter: string | null): Partial<Record<FilterKey, string>> {
  switch (filter as AdminUserFilter | null) {
    case 'active7d': return { seen: '7d' };
    case 'noAccountKey': return { key: 'missing' };
    case 'noTwoFactor': return { factor: 'none' };
    case 'disabled': return { status: 'disabled' };
    case 'neverSeen': return { seen: 'never' };
    default: return {};
  }
}

function UserList(props: AdminPageProps) {
  const { refreshKey, onLoaded, onUnauthorized, setParams, selectedId } = props;
  const values = useFilterValues(props, FILTER_KEYS, legacyFilter);
  const urlSort = useUrlSort(props);
  const settings = useTableSettings('users', { urlSort });
  const [text, setText] = useState('');
  const q = useDebounced(text.trim());
  const [page, setPage] = useState(1);
  const query = useMemo(() => ({
    q,
    role: values.role,
    status: values.status,
    key: values.key,
    factor: values.factor,
    method: values.method,
    ...windowParams(values.seen, 'seen'),
    ...windowParams(values.created, 'created'),
    sort: settings.sort?.key,
    dir: settings.sort?.dir,
  }), [q, values, settings.sort?.key, settings.sort?.dir]);
  const queryKey = JSON.stringify(query);
  useEffect(() => setPage(1), [queryKey]);
  const { data, error, loading, reload } = useAdminResource(
    () => adminApi.users({ ...query, page, pageSize: PAGE_SIZE }),
    `${queryKey}:${page}:${refreshKey}`,
    onUnauthorized,
  );
  useEffect(() => {
    if (data) onLoaded();
  }, [data, onLoaded]);
  const open = (user: AdminUser) => setParams({ id: user.id });
  const setFilter = (key: FilterKey, value: string | null) => setParams({ [key]: value, filter: null, id: null });
  const active = FILTER_KEYS.filter((key) => values[key]).length;
  const actions = useUserActions(() => {
    reload();
    props.onChanged();
  });
  const time = (value: string | null | undefined) => <time dateTime={value ?? undefined} title={formatDateTime(value ?? null)}>{formatRelative(value ?? null)}</time>;
  const columns: Array<TableColumn<AdminUser>> = [
    {
      key: 'name',
      label: translate('admin.user'),
      sortable: true,
      hideable: false,
      width: 240,
      minWidth: 160,
      render: (user) => (
        <RowOpener label={translate('admin.openUser', { name: user.username })} onOpen={() => open(user)}>
          <Avatar name={user.username} size="sm" />
          <span className="admin-two-line">
            <b>{user.username}</b>
            <span>{user.email}</span>
          </span>
        </RowOpener>
      ),
    },
    { key: 'email', label: translate('admin.email'), sortable: true, defaultHidden: true, width: 200, render: (user) => user.email },
    { key: 'role', label: translate('admin.role'), sortable: true, defaultHidden: true, width: 120, render: (user) => (user.role === 'admin' ? translate('admin.roleAdmin') : translate('admin.roleUser')) },
    { key: 'status', label: translate('admin.status'), sortable: true, width: 120, render: (user) => <UserStatus user={user} /> },
    { key: 'method', label: translate('admin.loginMethod'), sortable: true, defaultHidden: true, width: 150, render: (user) => user.loginMethods.map(loginMethodLabel).join(' · ') || '—' },
    { key: 'key', label: translate('admin.accountKey'), sortable: true, width: 130, render: (user) => <AccountKeyMark user={user} /> },
    { key: 'factors', label: translate('admin.twoFactor'), sortable: true, firstDir: 'desc', width: 140, render: (user) => factorsLabel(user) },
    { key: 'devices', label: translate('admin.devicesOnline'), sortable: true, firstDir: 'desc', numeric: true, defaultHidden: true, width: 120, render: (user) => `${formatCount(user.devicesOnline)} / ${formatCount(user.devices)}` },
    { key: 'created', label: translate('admin.registered'), sortable: true, firstDir: 'desc', width: 120, render: (user) => <time dateTime={user.createdAt} title={formatDateTime(user.createdAt)}>{formatDate(user.createdAt)}</time> },
    {
      key: 'lastSignIn',
      label: translate('admin.lastSignIn'),
      sortable: true,
      firstDir: 'desc',
      width: 170,
      render: (user) => (user.lastSignInAt ? (
        <span className="admin-two-line">
          <span className="admin-strong">{time(user.lastSignInAt)}</span>
          <span title={user.lastSignInBrowser ?? undefined}>{user.lastSignInBrowser ?? translate('admin.unknownBrowser')}</span>
        </span>
      ) : <span className="admin-muted">{translate('admin.never')}</span>),
    },
    { key: 'browser', label: translate('admin.lastSignInBrowser'), sortable: true, defaultHidden: true, width: 160, render: (user) => user.lastSignInBrowser ?? <span className="admin-muted">—</span> },
    { key: 'lastSeen', label: translate('admin.lastActive'), sortable: true, firstDir: 'desc', width: 130, render: (user) => time(user.lastSeenAt) },
    { key: 'menu', label: translate('admin.actions'), header: <span className="sr-only">{translate('admin.actions')}</span>, className: 'is-menu', hideable: false, fixed: true, width: 64, render: (user) => <RowMenu items={actions.menu(user)} label={translate('admin.userActions', { name: user.username })} /> },
  ];
  const counts = data?.counts;
  const methods = data?.methods;
  return (
    <>
      <div className="admin-toolbar">
        <FilterBar active={active} label={translate('admin.filterUsers')} onClear={() => setParams({ ...Object.fromEntries(FILTER_KEYS.map((key) => [key, null])), filter: null, id: null })}>
          <SelectFilter label={translate('admin.role')} onChange={(value) => setFilter('role', value)} options={[{ value: 'user', label: translate('admin.roleUser') }, { value: 'admin', label: translate('admin.roleAdmin') }]} value={values.role} />
          <SelectFilter
            label={translate('admin.status')}
            onChange={(value) => setFilter('status', value)}
            options={[
              { value: 'active', label: translate('admin.active') },
              { value: 'disabled', label: translate('admin.disabled'), count: counts?.disabled },
              { value: 'resetPending', label: translate('admin.resetPending') },
            ]}
            value={values.status}
          />
          <SelectFilter
            label={translate('admin.accountKey')}
            onChange={(value) => setFilter('key', value)}
            options={[
              { value: 'set', label: translate('admin.keySet'), count: methods?.accountKey },
              { value: 'missing', label: translate('admin.keyMissing'), count: counts?.noAccountKey },
            ]}
            value={values.key}
          />
          <SelectFilter
            label={translate('admin.twoFactor')}
            onChange={(value) => setFilter('factor', value)}
            options={[
              { value: 'any', label: translate('admin.anyTwoFactor') },
              { value: 'authenticator', label: translate('admin.authenticator'), count: methods?.authenticator },
              { value: 'passkey', label: translate('admin.passkey'), count: methods?.passkey },
              { value: 'none', label: translate('admin.none'), count: counts?.noTwoFactor },
            ]}
            value={values.factor}
          />
          <SelectFilter
            label={translate('admin.loginMethod')}
            onChange={(value) => setFilter('method', value)}
            options={[
              { value: 'password', label: translate('admin.methodPassword'), count: methods?.password },
              { value: 'google', label: 'Google', count: methods?.google },
              { value: 'github', label: 'GitHub', count: methods?.github },
              { value: 'oauthOnly', label: translate('admin.methodOauthOnly'), count: methods?.oauthOnly },
            ]}
            value={values.method}
          />
          <TimeFilter label={translate('admin.registered')} onChange={(value) => setFilter('created', value)} value={values.created} />
          <TimeFilter label={translate('admin.lastActive')} never older onChange={(value) => setFilter('seen', value)} value={values.seen} />
        </FilterBar>
        <SearchField label={translate('admin.searchUsers')} onChange={setText} placeholder={translate('admin.searchUsers')} value={text} />
      </div>
      {data && data.pendingRegistrations > 0 && !active ? (
        <button className="admin-callout" onClick={() => props.go('users', { sub: 'registrations' })} type="button">
          <UserPlus aria-hidden className="h-5 w-5" />
          <b>{translate('admin.pendingCallout', { count: formatCount(data.pendingRegistrations) })}</b>
          <span className="admin-callout-action">{translate('admin.review')}</span>
        </button>
      ) : null}
      {error && !data ? <LoadFailure message={error} onRetry={reload} /> : null}
      {!data && !error ? <Skeleton lines={8} /> : null}
      {data ? (
        <Card className="admin-table-card">
          <AdminTable
            busy={loading}
            columns={columns}
            empty={translate('admin.noUsersMatch')}
            id="users"
            label={translate('admin.navUsers')}
            onOpen={open}
            phoneRow={(user) => (
              <>
                <Avatar name={user.username} />
                <span className="admin-two-line">
                  <b>{user.username}</b>
                  <span>{user.enabled ? `${formatRelative(user.lastSeenAt)} · ${translate('admin.devicesOnlineShort', { online: user.devicesOnline, total: user.devices })}` : translate('admin.disabled')}</span>
                </span>
                <span className="admin-list-mark">
                  {!user.enabled ? <Badge icon={<Ban aria-hidden className="h-3.5 w-3.5" />} tone="danger">{translate('admin.disabled')}</Badge> : <AccountKeyMark iconOnly user={user} />}
                </span>
              </>
            )}
            rowKey={(user) => user.id}
            rows={data.items}
            selected={selectedId}
            settings={settings}
            total={data.total}
          />
          <Pagination onPage={setPage} page={data.page} pageSize={data.pageSize} total={data.total} />
        </Card>
      ) : null}
      <UserDetail actions={actions} id={selectedId} onClose={() => setParams({ id: null })} props={props} />
      {actions.dialog}
    </>
  );
}

export function UserStatus({ user }: { user: AdminUser }) {
  useI18n();
  if (!user.enabled) return <Badge icon={<Ban aria-hidden className="h-3.5 w-3.5" />} tone="danger">{translate('admin.disabled')}</Badge>;
  if (user.resetPending) return <Badge icon={<KeyRound aria-hidden className="h-3.5 w-3.5" />} tone="warning">{translate('admin.resetPending')}</Badge>;
  return <span className="admin-status-ok"><span aria-hidden className="admin-dot-mark" />{translate('admin.active')}</span>;
}

export function AccountKeyMark({ user, iconOnly = false }: { user: AdminUser; iconOnly?: boolean }) {
  useI18n();
  if (user.accountKey === null) return <span className="admin-muted">{translate('admin.notApplicable')}</span>;
  const text = user.accountKey ? translate('admin.keySet') : translate('admin.keyMissing');
  return (
    <span className={`admin-key-mark ${user.accountKey ? 'is-set' : 'is-missing'}`} title={iconOnly ? text : undefined}>
      <KeyRound aria-hidden className="h-4 w-4" />
      {iconOnly ? <span className="sr-only">{text}</span> : <span>{text}</span>}
    </span>
  );
}

function factorsLabel(user: AdminUser) {
  const parts = [user.authenticator ? translate('admin.authenticator') : null, user.passkeys ? translate('admin.passkeys', { count: user.passkeys }) : null].filter(Boolean);
  return parts.length ? parts.join(' + ') : <span className="admin-muted">{translate('admin.none')}</span>;
}

/** Enable/disable, temporary password, end sessions and delete, with the
    confirmations docs/ui-principles.md §11 asks for. */
function useUserActions(onDone: () => void) {
  const [resetTarget, setResetTarget] = useState<AdminUser | null>(null);
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  const run = async (user: AdminUser, action: () => Promise<unknown>) => {
    setFailure(null);
    try {
      await action();
      onDone();
      return true;
    } catch (caught) {
      setFailure({ id: user.id, message: errorText(caught) });
      return false;
    }
  };
  const toggle = async (user: AdminUser) => {
    if (user.enabled) {
      const confirmed = await confirmAsync({
        title: translate('admin.disableUserTitle', { name: user.username }),
        description: translate('admin.disableUserBody'),
        confirmLabel: translate('admin.disable'),
      });
      if (!confirmed) return;
    }
    await run(user, () => setRelayUserEnabled(user.id, !user.enabled));
  };
  const endSessions = async (user: AdminUser) => {
    const confirmed = await confirmAsync({
      title: translate('admin.endSessionsTitle', { name: user.username }),
      description: translate('admin.endSessionsBody'),
      confirmLabel: translate('admin.endAllSessions'),
    });
    if (confirmed) await run(user, () => adminApi.endUserSessions(user.id));
  };
  const remove = async (user: AdminUser) => {
    const confirmed = await confirmAsync({
      title: translate('admin.deleteUserTitle', { name: user.username }),
      description: translate('admin.deleteUserBody'),
      confirmLabel: translate('admin.deleteUser'),
    });
    if (confirmed) await run(user, () => deleteRelayAdminUser(user.id));
  };
  const menu = (user: AdminUser): MenuItem[] => {
    const admin = user.role === 'admin';
    return [
      { key: 'reset', label: translate('admin.temporaryPassword'), icon: <KeyRound aria-hidden className="h-4 w-4" />, disabled: admin, onSelect: () => setResetTarget(user) },
      { key: 'sessions', label: translate('admin.endAllSessions'), icon: <LogOut aria-hidden className="h-4 w-4" />, disabled: admin, onSelect: () => void endSessions(user) },
      { key: 'toggle', label: user.enabled ? translate('admin.disable') : translate('admin.enable'), icon: user.enabled ? <Ban aria-hidden className="h-4 w-4" /> : <UserCheck aria-hidden className="h-4 w-4" />, danger: user.enabled, disabled: admin, onSelect: () => void toggle(user) },
      { key: 'delete', label: translate('admin.deleteUser'), icon: <Trash2 aria-hidden className="h-4 w-4" />, danger: true, disabled: admin, onSelect: () => void remove(user) },
    ];
  };
  const dialog = resetTarget ? (
    <TemporaryPasswordDialog
      onClose={() => setResetTarget(null)}
      onSubmit={async (password) => {
        await resetRelayAdminUserPassword(resetTarget.id, password);
        setResetTarget(null);
        onDone();
      }}
      user={resetTarget}
    />
  ) : null;
  return { menu, toggle, endSessions, remove, reset: setResetTarget, dialog, failure, run };
}

type UserActions = ReturnType<typeof useUserActions>;

function TemporaryPasswordDialog({ user, onClose, onSubmit }: { user: AdminUser; onClose: () => void; onSubmit: (password: string) => Promise<void> }) {
  useI18n();
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const tooShort = password.length > 0 && password.length < 12;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (password.length < 12) return;
    setBusy(true);
    setFailure(null);
    try {
      await onSubmit(password);
    } catch (caught) {
      setFailure(errorText(caught));
    } finally {
      setBusy(false);
    }
  };
  return (
    <FormDialog busy={busy} description={translate('admin.temporaryPasswordBody', { name: user.username })} onClose={onClose} title={translate('admin.temporaryPassword')}>
      <form className="admin-form" onSubmit={submit}>
        <label>
          <span>{translate('admin.temporaryPasswordLabel')}</span>
          <input autoComplete="new-password" className="relay-input" minLength={12} onChange={(event) => setPassword(event.target.value)} type="password" value={password} />
        </label>
        {tooShort ? <p className="admin-field-hint">{translate('admin.passwordTooShort')}</p> : null}
        <div className="admin-form-actions">
          <button className="relay-button-secondary" disabled={busy} onClick={onClose} type="button">{translate('admin.cancel')}</button>
          <span className="admin-inline">
            <button className="relay-button-primary" disabled={busy || password.length < 12} type="submit">
              <KeyRound aria-hidden className="mr-2 h-4 w-4" />
              {busy ? translate('admin.saving') : translate('admin.setTemporaryPassword')}
            </button>
            {failure ? <ErrorMark message={failure} /> : null}
          </span>
        </div>
      </form>
    </FormDialog>
  );
}

function UserDetail({ id, onClose, props, actions }: { id: string | null; onClose: () => void; props: AdminPageProps; actions: UserActions }) {
  useI18n();
  const { data, error, reload } = useAdminResource<AdminUserDetail | null>(
    () => (id ? adminApi.user(id) : Promise.resolve(null)),
    `${id}:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const [ending, setEnding] = useState<string | null>(null);
  const [sessionFailure, setSessionFailure] = useState<{ id: string; message: string } | null>(null);
  const detail = data && data.user.id === id ? data : null;
  const user = detail?.user;
  const endSession = async (session: AdminSession) => {
    const confirmed = await confirmAsync({
      title: translate('admin.endSessionTitle'),
      description: translate('admin.endSessionBody', { environment: session.environment ?? translate('admin.unknownBrowser') }),
      confirmLabel: translate('admin.endSession'),
    });
    if (!confirmed) return;
    setEnding(session.id);
    setSessionFailure(null);
    try {
      await adminApi.endSession(session.id);
      reload();
    } catch (caught) {
      setSessionFailure({ id: session.id, message: errorText(caught) });
    } finally {
      setEnding(null);
    }
  };
  const after = () => reload();
  const admin = user?.role === 'admin';
  return (
    <DetailWindow
      actions={
        user ? (
          <>
            <button className="relay-button-secondary" disabled={admin} onClick={() => void actions.endSessions(user).then(after)} type="button">
              <LogOut aria-hidden className="mr-2 h-4 w-4" />
              {translate('admin.endAllSessions')}
            </button>
            <button className="relay-button-secondary" disabled={admin} onClick={() => actions.reset(user)} type="button">
              <KeyRound aria-hidden className="mr-2 h-4 w-4" />
              {translate('admin.temporaryPassword')}
            </button>
            <span className="admin-actions-spacer" />
            <button className={user.enabled ? 'ui-action-danger admin-action-button' : 'relay-button-secondary'} disabled={admin} onClick={() => void actions.toggle(user).then(after)} type="button">
              {user.enabled ? <Ban aria-hidden className="mr-2 h-4 w-4" /> : <UserCheck aria-hidden className="mr-2 h-4 w-4" />}
              {user.enabled ? translate('admin.disable') : translate('admin.enable')}
            </button>
            {actions.failure?.id === user.id ? <ErrorMark message={actions.failure.message} /> : null}
            <RowMenu items={actions.menu(user).filter((item) => item.key === 'delete')} label={translate('admin.moreActions')} />
          </>
        ) : null
      }
      badges={
        user ? (
          <>
            <UserStatus user={user} />
            <AccountKeyMark user={user} />
            {user.authenticator || user.passkeys ? <Badge icon={<Fingerprint aria-hidden className="h-3.5 w-3.5" />} tone="neutral">{translate('admin.twoFactor')}</Badge> : null}
          </>
        ) : null
      }
      icon={user ? <Avatar name={user.username} size="lg" /> : null}
      onClose={onClose}
      open={Boolean(id)}
      subtitle={user ? translate('admin.userSubtitle', { email: user.email, date: formatDate(user.createdAt) }) : null}
      title={user?.username ?? translate('admin.loading')}
    >
      {!detail ? (error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={6} />) : (
        <>
          <div className="admin-mini-stats">
            <MiniStat label={translate('admin.devicesOnline')} value={`${formatCount(detail.user.devicesOnline)} / ${formatCount(detail.user.devices)}`} />
            <MiniStat label={translate('admin.sharesOutIn')} value={`${formatCount(detail.shares.outgoing)} · ${formatCount(detail.shares.incoming)}`} />
            <MiniStat label={translate('admin.activeSessions')} value={formatCount(detail.sessions.length)} />
          </div>
          <DetailSection icon={<MonitorSmartphone aria-hidden className="h-4 w-4" />} title={translate('admin.navDevices')}>
            {detail.devices.length ? (
              <ul className="admin-detail-list">
                {detail.devices.map((device) => (
                  <li key={device.id}>
                    <button className="admin-detail-link" onClick={() => props.go('devices', { id: device.id })} type="button">
                      <span className="admin-two-line">
                        <b>{device.name}</b>
                        <span>{[device.version, translate('admin.threadsCount', { count: device.threadCount })].filter(Boolean).join(' · ')}</span>
                      </span>
                    </button>
                    <span className="admin-detail-marks">
                      <ProtectionMark protectedDevice={device.protected} />
                      <DeviceStateMark state={device.connected ? (device.unstable ? 'unstable' : 'online') : 'offline'} />
                    </span>
                  </li>
                ))}
              </ul>
            ) : <EmptyState>{translate('admin.noDevices')}</EmptyState>}
            {(detail.devicesTotal ?? 0) > detail.devices.length ? (
              <button className="admin-link-button" onClick={() => props.go('devices', { params: { owner: detail.user.id } })} type="button">
                {translate('admin.allDevicesOf', { count: detail.devicesTotal ?? 0 })}
              </button>
            ) : null}
          </DetailSection>
          <DetailSection icon={<ShieldCheck aria-hidden className="h-4 w-4" />} title={translate('admin.sessions')}>
            {detail.sessions.length ? (
              <ul className="admin-detail-list">
                {detail.sessions.map((session) => (
                  <li key={session.id}>
                    <span className="admin-two-line">
                      <b>{session.environment ?? translate('admin.unknownBrowser')}</b>
                      <span>
                        {translate('admin.signedInAt', { time: formatRelative(session.createdAt) })}
                        {' · '}
                        {session.trustedBrowser ? translate('admin.trustedBrowser') : translate('admin.expiresIn', { time: formatRelative(session.expiresAt) })}
                      </span>
                    </span>
                    <span className="admin-inline">
                      <button className="admin-text-button is-danger" disabled={admin || ending === session.id} onClick={() => void endSession(session)} type="button">
                        {translate('admin.endSession')}
                      </button>
                      {sessionFailure?.id === session.id ? <ErrorMark message={sessionFailure.message} /> : null}
                    </span>
                  </li>
                ))}
              </ul>
            ) : <EmptyState>{translate('admin.noSessions')}</EmptyState>}
          </DetailSection>
          <DetailSection icon={<History aria-hidden className="h-4 w-4" />} title={translate('admin.recentEvents')}>
            <EventList compact events={detail.events} links={{ onOpenDevice: (device) => props.go('devices', { id: device }), onOpenUser: (user) => props.go('users', { id: user }) }} />
          </DetailSection>
          {detail.registration ? (
            <p className="admin-detail-note">
              {translate('admin.approvedBy', { by: detail.registration.approvedBy ?? '—', time: formatDateTime(detail.registration.approvedAt) })}
            </p>
          ) : null}
        </>
      )}
    </DetailWindow>
  );
}

function Registrations(props: AdminPageProps) {
  useI18n();
  const { refreshKey, onUnauthorized, onLoaded } = props;
  const [page, setPage] = useState(1);
  const urlSort = useUrlSort(props);
  const settings = useTableSettings('registrations', { urlSort });
  const sortKey = `${settings.sort?.key ?? ''}:${settings.sort?.dir ?? ''}`;
  useEffect(() => setPage(1), [sortKey]);
  const pending = useAdminResource(() => adminApi.registrations({ status: 'pending', pageSize: 100 }), `pending:${refreshKey}`, onUnauthorized);
  const reviewed = useAdminResource(() => adminApi.registrations({ status: 'reviewed', page, pageSize: 15, sort: settings.sort?.key, dir: settings.sort?.dir }), `reviewed:${page}:${sortKey}:${refreshKey}`, onUnauthorized);
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<{ id: string; message: string } | null>(null);
  useEffect(() => {
    if (pending.data) onLoaded();
  }, [pending.data, onLoaded]);
  const review = async (request: AdminRegistration, action: 'approve' | 'reject') => {
    const values = { name: request.username, email: request.email };
    const confirmed = await confirmAsync(action === 'approve'
      ? { title: translate('admin.approveTitle'), description: translate('admin.approveBody', values), confirmLabel: translate('admin.approve'), tone: 'primary' }
      : { title: translate('admin.rejectTitle'), description: translate('admin.rejectBody', values), confirmLabel: translate('admin.reject') });
    if (!confirmed) return;
    setBusy(request.id);
    setFailure(null);
    try {
      if (action === 'approve') await approveRelayRegistration(request.id);
      else await rejectRelayRegistration(request.id);
      pending.reload();
      reviewed.reload();
      props.onChanged();
    } catch (caught) {
      setFailure({ id: request.id, message: errorText(caught) });
    } finally {
      setBusy(null);
    }
  };
  const summary = pending.data?.summary;
  return (
    <>
      {summary ? (
        <div className="admin-mini-stats is-band">
          <MiniStat label={translate('admin.pendingRegistrations')} value={formatCount(summary.pending)} />
          <MiniStat label={translate('admin.approved30d')} value={formatCount(summary.approved30d)} />
          <MiniStat label={translate('admin.rejected30d')} value={formatCount(summary.rejected30d)} />
          <MiniStat label={translate('admin.medianApprovalShort')} value={summary.medianApprovalHours === null ? '—' : formatDuration(summary.medianApprovalHours * 3600)} />
        </div>
      ) : null}
      <Card subtitle={translate('admin.pendingSubtitle')} title={translate('admin.pendingRegistrations')}>
        {pending.error && !pending.data ? <LoadFailure message={pending.error} onRetry={pending.reload} /> : null}
        {pending.data ? (
          pending.data.items.length ? (
            <ul className="admin-review-list">
              {pending.data.items.map((request) => (
                <li key={request.id}>
                  <Avatar name={request.username} size="sm" />
                  <span className="admin-two-line">
                    <b>{request.username}</b>
                    <span>{request.email} · {loginMethodLabel(request.provider)} · {formatRelative(request.createdAt)}</span>
                  </span>
                  <span className="admin-review-actions">
                    <button className="relay-button-secondary" disabled={busy !== null} onClick={() => void review(request, 'reject')} type="button">
                      {translate('admin.reject')}
                    </button>
                    <button className="relay-button-primary" disabled={busy !== null} onClick={() => void review(request, 'approve')} type="button">
                      <Check aria-hidden className="mr-1.5 h-4 w-4" />
                      {translate('admin.approve')}
                    </button>
                    {failure?.id === request.id ? <ErrorMark message={failure.message} /> : null}
                  </span>
                </li>
              ))}
            </ul>
          ) : <EmptyState>{translate('admin.noPending')}</EmptyState>
        ) : !pending.error ? <Skeleton lines={3} /> : null}
      </Card>
      <Card title={translate('admin.reviewHistory')}>
        {reviewed.data ? (
          <>
            <AdminTable
              columns={[
                { key: 'user', label: translate('admin.user'), sortable: true, hideable: false, width: 240, render: (row) => <span className="admin-two-line"><b>{row.username}</b><span>{row.email}</span></span> },
                { key: 'result', label: translate('admin.result'), sortable: true, width: 130, render: (row) => row.status === 'approved' ? <Badge tone="success">{translate('admin.approved')}</Badge> : <Badge tone="danger">{translate('admin.rejected')}</Badge> },
                { key: 'method', label: translate('admin.loginMethod'), width: 130, defaultHidden: true, render: (row) => loginMethodLabel(row.provider) },
                { key: 'requested', label: translate('admin.requested'), sortable: true, firstDir: 'desc', width: 180, render: (row) => formatDateTime(row.createdAt) },
                { key: 'reviewed', label: translate('admin.reviewed'), sortable: true, firstDir: 'desc', width: 240, render: (row) => `${formatDateTime(row.reviewedAt)} · ${row.reviewedBy ?? '—'}` },
              ]}
              empty={translate('admin.noReviews')}
              id="registrations"
              label={translate('admin.reviewHistory')}
              settings={settings}
              phoneRow={(row) => (
                <>
                  <span className="admin-two-line">
                    <b>{row.username}</b>
                    <span>{formatRelative(row.reviewedAt)} · {row.reviewedBy ?? '—'}</span>
                  </span>
                  {row.status === 'approved' ? <Badge tone="success">{translate('admin.approved')}</Badge> : <Badge tone="danger">{translate('admin.rejected')}</Badge>}
                </>
              )}
              rowKey={(row) => row.id}
              rows={reviewed.data.items}
            />
            <Pagination onPage={setPage} page={reviewed.data.page} pageSize={reviewed.data.pageSize} total={reviewed.data.total} />
          </>
        ) : reviewed.error ? <LoadFailure message={reviewed.error} onRetry={reviewed.reload} /> : <Skeleton lines={3} />}
      </Card>
    </>
  );
}

function Methods(props: AdminPageProps) {
  useI18n();
  const { data, error, reload } = useAdminResource(() => adminApi.users({ pageSize: 1 }), `methods:${props.refreshKey}`, props.onUnauthorized);
  const loaded = useRef(props.onLoaded);
  loaded.current = props.onLoaded;
  useEffect(() => {
    if (data) loaded.current();
  }, [data]);
  if (!data) return error ? <LoadFailure message={error} onRetry={reload} /> : <Skeleton lines={5} />;
  const methods = data.methods;
  const total = methods.users;
  return (
    <div className="admin-grid is-halves">
      <ChartCard
        subtitle={translate('admin.methodsSubtitle', { total: formatCount(total) })}
        table={{ columns: [translate('admin.loginMethod'), translate('admin.people')], rows: [[translate('admin.methodPassword'), methods.password], ['Google', methods.google], ['GitHub', methods.github], [translate('admin.methodOauthOnly'), methods.oauthOnly]] }}
        title={translate('admin.loginMethods')}
      >
        <BarList
          items={[
            { key: 'password', label: translate('admin.methodPassword'), value: methods.password },
            { key: 'google', label: 'Google', value: methods.google },
            { key: 'github', label: 'GitHub', value: methods.github },
            { key: 'oauthOnly', label: translate('admin.methodOauthOnly'), value: methods.oauthOnly },
          ]}
          label={translate('admin.loginMethods')}
          max={total}
        />
      </ChartCard>
      <ChartCard
        subtitle={translate('admin.factorsSubtitle')}
        table={{ columns: [translate('admin.protection'), translate('admin.people')], rows: [[translate('admin.accountKey'), methods.accountKey], [translate('admin.anyTwoFactor'), methods.twoFactor], [translate('admin.authenticator'), methods.authenticator], [translate('admin.passkey'), methods.passkey]] }}
        title={translate('admin.protection')}
      >
        <BarList
          items={[
            { key: 'key', label: translate('admin.accountKey'), value: methods.accountKey },
            { key: 'any', label: translate('admin.anyTwoFactor'), value: methods.twoFactor },
            { key: 'totp', label: translate('admin.authenticator'), value: methods.authenticator },
            { key: 'passkey', label: translate('admin.passkey'), value: methods.passkey },
          ]}
          label={translate('admin.protection')}
          max={total}
        />
      </ChartCard>
    </div>
  );
}
