import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayPortalSummaryDto } from '@pockymoe/shared';
import {
  ArrowLeft,
  ArrowUpRight,
  ChevronDown,
  ChevronRight,
  Eye,
  FilePen,
  FileText,
  Folder,
  History,
  Hourglass,
  Inbox,
  KeyRound,
  List,
  LogOut,
  MessageSquare,
  Monitor,
  MonitorSmartphone,
  Pencil,
  Plus,
  Search,
  Send,
  ShieldAlert,
  SlidersHorizontal,
  UserRoundMinus,
  Users,
  X,
} from 'lucide-react';
import { useEffect, useId, useMemo, useState, type ReactNode } from 'react';
import { AccountSafetyCode, SignedShareDetails } from '../SignedShareDetails';
import {
  filterItems,
  groupByDevice,
  groupByPerson,
  initials,
  scopeCounts,
  sharedItems,
  type PersonNode,
  type ShareDirection,
  type ShareFilter,
  type SharedItem,
  type SharedView,
  type ShareScope,
} from './model';
import type { AttentionState } from './useShareAttention';
import './shared-access.css';

const VIEW_KEY = 'pockymoe.sharedAccess.view';
const SCOPE_ICON: Record<ShareScope, typeof Monitor> = { device: Monitor, workspace: Folder, thread: MessageSquare };
const ATTENTION_TEXT: Record<AttentionState, Parameters<typeof translate>[0]> = {
  pending: 'sharing.attentionPending',
  missing: 'sharing.attentionMissing',
  different: 'sharing.attentionDifferent',
  changed: 'sharing.attentionChanged',
  unverified: 'sharing.attentionUnverified',
};

function readView(): SharedView {
  try {
    const value = localStorage.getItem(VIEW_KEY);
    return value === 'devices' || value === 'people' ? value : 'list';
  } catch {
    return 'list';
  }
}

const formatDate = (value: string) => new Date(value).toLocaleString(getLocale(), { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

function useWide() {
  const query = '(min-width: 768px)';
  const [wide, setWide] = useState(() => typeof window === 'undefined' || !window.matchMedia || window.matchMedia(query).matches);
  useEffect(() => {
    if (!window.matchMedia) return;
    const media = window.matchMedia(query);
    const update = () => setWide(media.matches);
    update();
    media.addEventListener?.('change', update);
    return () => media.removeEventListener?.('change', update);
  }, []);
  return wide;
}

export interface SharedAccessActions {
  onOpen: (item: SharedItem) => void;
  onEdit: (item: SharedItem) => void;
  onRevoke: (item: SharedItem) => void;
  /** Give up something shared with me (asks first). */
  onLeave: (item: SharedItem) => void;
  /** Something shared with me that waits for the owner's safety-code check:
      the page with my code. */
  onShowCode: (item: SharedItem) => void;
}

/** Shares and grants in both directions, as one list, by device or by person
    (docs/account-key-trust.md#sharing). Each row is one share; details and
    actions open in place. */
export function SharedAccessPanel({
  portal,
  loading,
  direction,
  onDirectionChange,
  attention,
  onAttentionRefresh,
  busyKey,
  actions,
  outgoingNotice,
}: {
  portal: RelayPortalSummaryDto | null;
  loading: boolean;
  direction: ShareDirection;
  onDirectionChange: (direction: ShareDirection) => void;
  attention: ReadonlyMap<string, AttentionState>;
  onAttentionRefresh: () => void;
  busyKey: string | null;
  actions: SharedAccessActions;
  /** Grants a device still holds without a relay record (my shares only). */
  outgoingNotice?: ReactNode;
}) {
  useI18n();
  const [view, setViewState] = useState<SharedView>(readView);
  const [filter, setFilter] = useState<ShareFilter>('all');
  const [query, setQuery] = useState('');
  const [expanded, setExpanded] = useState<string | null>(null);
  const incoming = useMemo(() => sharedItems(portal, 'in'), [portal]);
  const outgoing = useMemo(() => sharedItems(portal, 'out'), [portal]);
  const all = direction === 'in' ? incoming : outgoing;
  const visible = useMemo(() => filterItems(all, filter, query, attention), [all, filter, query, attention]);
  const counts = scopeCounts(all);
  const attentionCount = all.filter(item => attention.has(item.key)).length;
  useEffect(() => { setExpanded(null); }, [direction, view]);
  useEffect(() => {
    if (filter === 'attention' && !attentionCount) setFilter('all');
  }, [attentionCount, filter]);
  const setView = (next: SharedView) => {
    setViewState(next);
    try { localStorage.setItem(VIEW_KEY, next); } catch { /* The choice lasts for this visit. */ }
  };
  const toggle = (key: string) => setExpanded(current => {
    // Leaving a share's details picks up what was confirmed there.
    if (current) onAttentionRefresh();
    return current === key ? null : key;
  });
  const rowProps = { attention, busyKey, actions, expanded, onToggle: toggle };
  const views: Array<{ id: SharedView; label: string; Icon: typeof List }> = [
    { id: 'list', label: translate('sharing.viewList'), Icon: List },
    { id: 'devices', label: translate('sharing.viewDevices'), Icon: MonitorSmartphone },
    { id: 'people', label: translate('sharing.viewPeople'), Icon: Users },
  ];
  const filters: Array<{ id: ShareFilter; label: string; count?: number; Icon?: typeof List }> = [
    { id: 'all', label: translate('sharing.filterAll') },
    { id: 'device', label: translate('sharing.filterDevices'), count: counts.device, Icon: Monitor },
    { id: 'workspace', label: translate('sharing.filterWorkspaces'), count: counts.workspace, Icon: Folder },
    { id: 'thread', label: translate('sharing.filterThreads'), count: counts.thread, Icon: MessageSquare },
    ...(attentionCount ? [{ id: 'attention' as const, label: translate('sharing.filterAttention'), count: attentionCount, Icon: ShieldAlert }] : []),
  ];

  return (
    <div className="shared-panel" data-view={view} data-direction={direction}>
      <div className="shared-panel-toolbar">
        <div className="shared-panel-segment" role="group" aria-label={translate('sharing.direction')}>
          {([['in', translate('sharing.withMe'), incoming.length, Inbox], ['out', translate('sharing.byMe'), outgoing.length, Send]] as const).map(([id, label, count, Icon]) => (
            <button key={id} type="button" aria-pressed={direction === id} onClick={() => onDirectionChange(id)}>
              <Icon size={15} aria-hidden="true" /><span>{label}</span><span className="shared-panel-count">{count}</span>
            </button>
          ))}
        </div>
        <div className="shared-panel-segment shared-panel-views" role="group" aria-label={translate('sharing.viewLabel')}>
          {views.map(({ id, label, Icon }) => (
            <button key={id} type="button" aria-pressed={view === id} onClick={() => setView(id)} title={label}>
              <Icon size={15} aria-hidden="true" /><span className="shared-panel-view-label">{label}</span>
            </button>
          ))}
        </div>
        <label className="shared-panel-search">
          <Search size={15} aria-hidden="true" />
          <input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder={translate('sharing.searchShares')} aria-label={translate('sharing.searchShares')} />
          {query ? <button type="button" aria-label={translate('sharing.clearSearch')} onClick={() => setQuery('')}><X size={14} /></button> : null}
        </label>
      </div>
      <div className="shared-panel-filters" role="group" aria-label={translate('sharing.filterLabel')}>
        {filters.map(({ id, label, count, Icon }) => (
          <button key={id} type="button" aria-pressed={filter === id} data-tone={id === 'attention' ? 'warn' : undefined} onClick={() => setFilter(id)}>
            {Icon ? <Icon size={13} aria-hidden="true" /> : null}{label}{count !== undefined ? <span>{count}</span> : null}
          </button>
        ))}
      </div>
      {direction === 'out' ? outgoingNotice : null}
      {loading && !portal ? (
        <p className="shared-panel-empty">{translate('sharing.loadingShares')}</p>
      ) : !all.length ? (
        <p className="shared-panel-empty">{translate(direction === 'in' ? 'sharing.nothingSharedWithMe' : 'sharing.nothingSharedByMe')}</p>
      ) : !visible.length ? (
        <p className="shared-panel-empty">{translate('sharing.noMatchingShares')}</p>
      ) : view === 'list' ? (
        <div className="shared-list">{visible.map(item => <ShareRow key={item.key} item={item} {...rowProps} />)}</div>
      ) : view === 'devices' ? (
        <DeviceView items={visible} {...rowProps} />
      ) : (
        <PeopleView items={visible} direction={direction} {...rowProps} />
      )}
    </div>
  );
}

interface RowProps {
  attention: ReadonlyMap<string, AttentionState>;
  busyKey: string | null;
  actions: SharedAccessActions;
  expanded: string | null;
  onToggle: (key: string) => void;
}

function Presence({ connected, name }: { connected: boolean | undefined; name?: string }) {
  const label = translate(connected === true ? 'sharing.online' : connected === false ? 'sharing.offline' : 'sharing.unknown');
  return (
    <span className="shared-presence" data-online={connected === true ? 'true' : connected === false ? 'false' : undefined}>
      <span className="shared-presence-dot" role="img" aria-label={label} title={label} />
      {name ? <span className="shared-presence-name">{name}</span> : null}
    </span>
  );
}

function AccessPills({ item }: { item: SharedItem }) {
  return (
    <span className="shared-pills">
      <span className="shared-pill">{item.threadAccess === 'control' ? <Pencil size={12} aria-hidden="true" /> : <Eye size={12} aria-hidden="true" />}{translate(item.threadAccess === 'control' ? 'devices.collaborator' : 'devices.viewOnly')}</span>
      {item.workspaceAccess !== 'none' ? <span className="shared-pill">{item.workspaceAccess === 'write' ? <FilePen size={12} aria-hidden="true" /> : <FileText size={12} aria-hidden="true" />}{translate(item.workspaceAccess === 'write' ? 'sharing.filesWrite' : 'sharing.filesRead')}</span> : null}
      {item.canCreateThreads ? <span className="shared-pill"><Plus size={12} aria-hidden="true" />{translate('sharing.newThreadsShort')}</span> : null}
    </span>
  );
}

function Avatar({ name, size = 'sm' }: { name: string; size?: 'sm' | 'md' | 'lg' }) {
  return <span className="shared-avatar" data-size={size} aria-hidden="true">{initials(name)}</span>;
}

function subtitle(item: SharedItem, { showDevice = true, showPerson = true, showWorkspace = true }: { showDevice?: boolean; showPerson?: boolean; showWorkspace?: boolean }) {
  const parts: ReactNode[] = [];
  if (showWorkspace && item.scope === 'thread' && item.workspaceLabel) parts.push(item.workspaceLabel);
  if (item.scope === 'device' && item.selectedWorkspaces !== null) parts.push(translate('sharing.selectedWorkspaces', { count: item.selectedWorkspaces }));
  if (showDevice) parts.push(<Presence key="device" connected={item.deviceConnected} name={item.scope === 'device' ? translate(item.deviceConnected === true ? 'sharing.online' : item.deviceConnected === false ? 'sharing.offline' : 'sharing.unknown') : item.deviceName} />);
  if (showPerson && item.direction === 'in') parts.push(translate('sharing.fromPerson', { name: item.personName }));
  if (item.direction === 'out') parts.push(item.lastAccessedAt ? translate('sharing.visited', { value1: formatDate(item.lastAccessedAt) }) : translate('sharing.noVisitsYet'));
  return parts.map((part, index) => <span key={index} className="shared-sub-part">{part}</span>);
}

function ShareRow({ item, attention, busyKey, actions, expanded, onToggle, depth = 0, showDevice = true, showPerson = true, title = item.title }: RowProps & { item: SharedItem; depth?: 0 | 1; showDevice?: boolean; showPerson?: boolean; title?: string }) {
  // A thread nested under its workspace does not name the workspace again.
  const showWorkspace = depth === 0;
  const open = expanded === item.key;
  const detailId = useId();
  const Icon = SCOPE_ICON[item.scope] ?? MessageSquare;
  const flag = attention.get(item.key);
  // Shared with me, waiting for the owner to confirm my safety code.
  const waiting = item.direction === 'in' && item.source.row.pending === true;
  const kindLabel = translate(item.scope === 'device' ? 'sharing.kindDevice' : item.scope === 'workspace' ? 'sharing.kindWorkspace' : 'sharing.kindThread');
  return (
    <div className="shared-row" data-open={open || undefined} data-depth={depth || undefined}>
      <div className="shared-row-line">
        <button type="button" className="shared-row-toggle" aria-expanded={open} aria-controls={detailId} onClick={() => onToggle(item.key)}>
          <span className="shared-kind" data-scope={item.scope} role="img" aria-label={kindLabel} title={kindLabel}><Icon size={16} /></span>
          <span className="shared-row-main">
            <span className="shared-row-title">{title}</span>
            <span className="shared-row-sub">{subtitle(item, { showDevice, showPerson, showWorkspace })}</span>
          </span>
          <span className="shared-row-access"><AccessPills item={item} /></span>
          {flag ? <span className="shared-flag" title={translate(ATTENTION_TEXT[flag])}>{flag === 'pending' ? <Hourglass size={13} aria-hidden="true" /> : <ShieldAlert size={13} aria-hidden="true" />}<span>{translate(ATTENTION_TEXT[flag])}</span></span> : null}
          {waiting ? <span className="shared-flag" title={translate('sharing.incomingPending', { name: item.personName })}><Hourglass size={13} aria-hidden="true" /><span>{translate('sharing.incomingPending', { name: item.personName })}</span></span> : null}
          {item.direction === 'out' && showPerson ? <span className="shared-row-person" title={translate('sharing.sharedWithPerson', { name: item.personName })}><Avatar name={item.personName} /><span className="shared-row-person-name">{item.personName}</span></span> : null}
          <ChevronDown className="shared-row-chevron" size={16} aria-hidden="true" />
        </button>
        {waiting ? (
          <button type="button" className="shared-open" aria-label={translate('sharing.showMyCode')} title={translate('sharing.showMyCode')} onClick={() => actions.onShowCode(item)}>
            <KeyRound size={14} aria-hidden="true" /><span>{translate('sharing.showMyCode')}</span>
          </button>
        ) : item.direction === 'in' ? (
          <button type="button" className="shared-open" aria-label={translate('sharing.openNamed', { name: item.title })} disabled={item.deviceConnected !== true} title={item.deviceConnected === true ? translate('sharing.open') : translate('sharing.supervisorOfflineReconnectTheDeviceToOpen')} onClick={() => actions.onOpen(item)}>
            <span>{translate('sharing.open')}</span><ArrowUpRight size={14} aria-hidden="true" />
          </button>
        ) : null}
      </div>
      {open ? <ShareDetail id={detailId} item={item} busy={busyKey === item.key} actions={actions} /> : null}
    </div>
  );
}

function ShareDetail({ id, item, busy, actions }: { id: string; item: SharedItem; busy: boolean; actions: SharedAccessActions }) {
  const [history, setHistory] = useState(false);
  const waiting = item.direction === 'in' && item.source.row.pending === true;
  const events = item.source.row.accessEvents ?? [];
  return (
    <div className="shared-detail" id={id}>
      {/* My shares say what they need in their signed details below, once. */}
      {waiting ? (
        <div className="shared-detail-waiting">
          <p className="shared-detail-flag" role="status"><Hourglass size={14} aria-hidden="true" />{translate('sharing.incomingPendingText', { name: item.personName })}</p>
          <AccountSafetyCode />
        </div>
      ) : null}
      <dl className="shared-detail-grid">
        <div><dt>{translate(item.direction === 'in' ? 'sharing.sharedBy' : 'sharing.sharedWith')}</dt><dd><Avatar name={item.personName} />{item.personName}</dd></div>
        <div><dt>{translate('sharing.permissions')}</dt><dd><AccessPills item={item} /></dd></div>
        <div><dt>{translate('sharing.deviceLabel')}</dt><dd><Presence connected={item.deviceConnected} name={item.deviceName} /></dd></div>
        {item.workspaceLabel && item.scope !== 'device' ? <div><dt>{translate('sharing.kindWorkspace')}</dt><dd>{item.workspaceLabel}</dd></div> : null}
        {item.label && item.label !== item.title ? <div><dt>{translate('sharing.labelField')}</dt><dd>{item.label}</dd></div> : null}
        {item.direction === 'out' ? <div><dt>{translate('sharing.lastVisit')}</dt><dd>{item.lastAccessedAt ? formatDate(item.lastAccessedAt) : translate('sharing.noVisitsYet')}</dd></div> : null}
        <div><dt>{translate('sharing.createdAt')}</dt><dd>{formatDate(item.createdAt)}</dd></div>
        {item.expiresAt ? <div><dt>{translate('sharing.expiresAt')}</dt><dd>{formatDate(item.expiresAt)}</dd></div> : null}
      </dl>
      {item.direction === 'out' ? <SignedShareDetails kind={item.source.kind} row={item.source.row} /> : null}
      {history ? (
        <section className="shared-history" aria-label={translate('sharing.recentAccess')}>
          {events.length ? (
            <ol>{events.map(event => <li key={event.id}><span>{event.username}<small>{event.kind.replaceAll('_', ' ')}</small></span><time dateTime={event.accessedAt}>{formatDate(event.accessedAt)}</time></li>)}</ol>
          ) : <p>{translate('sharing.noVisitsRecordedYet')}</p>}
        </section>
      ) : null}
      <div className="shared-detail-actions">
        {item.direction === 'in' ? (
          <>
            {waiting
              ? <button type="button" className="shared-action" data-tone="primary" onClick={() => actions.onShowCode(item)}><KeyRound size={15} aria-hidden="true" />{translate('sharing.showMyCode')}</button>
              : <button type="button" className="shared-action" data-tone="primary" disabled={item.deviceConnected !== true} onClick={() => actions.onOpen(item)}><ArrowUpRight size={15} aria-hidden="true" />{translate('sharing.open')}</button>}
            <span className="shared-detail-spacer" />
            <button type="button" className="shared-action" data-tone="danger" disabled={busy} onClick={() => actions.onLeave(item)}><LogOut size={15} aria-hidden="true" />{translate('sharing.leaveAccess')}</button>
          </>
        ) : (
          <>
            <button type="button" className="shared-action" disabled={busy} onClick={() => actions.onEdit(item)}><SlidersHorizontal size={15} aria-hidden="true" />{translate('sharing.editPermissions')}</button>
            <button type="button" className="shared-action" aria-expanded={history} onClick={() => setHistory(value => !value)}><History size={15} aria-hidden="true" />{translate('sharing.accessHistory')}</button>
            <span className="shared-detail-spacer" />
            <button type="button" className="shared-action" data-tone="danger" disabled={busy} onClick={() => actions.onRevoke(item)}><UserRoundMinus size={15} aria-hidden="true" />{translate('sharing.revoke')}</button>
          </>
        )}
      </div>
    </div>
  );
}

function DeviceView({ items, ...rowProps }: RowProps & { items: SharedItem[] }) {
  const groups = useMemo(() => groupByDevice(items), [items]);
  return (
    <div className="shared-groups">
      {groups.map(group => {
        const direction = group.items[0]!.direction;
        const owners = [...new Set(group.items.map(item => item.personName))];
        return (
          <section key={group.deviceId} className="shared-group" aria-label={group.deviceName}>
            <header className="shared-group-head">
              <span className="shared-kind" data-scope="device"><Monitor size={16} aria-hidden="true" /></span>
              <span className="shared-row-main">
                <span className="shared-row-title">{group.deviceName}</span>
                <span className="shared-row-sub">
                  <span className="shared-sub-part"><Presence connected={group.deviceConnected} name={translate(group.deviceConnected === true ? 'sharing.online' : group.deviceConnected === false ? 'sharing.offline' : 'sharing.unknown')} /></span>
                  <span className="shared-sub-part">{direction === 'in' ? translate('sharing.fromPerson', { name: owners.join(', ') }) : translate('sharing.sharedCountPeople', { count: group.items.length, people: owners.length })}</span>
                </span>
              </span>
              {direction === 'out' ? <span className="shared-avatars" title={owners.join(', ')}>{owners.slice(0, 4).map(name => <Avatar key={name} name={name} />)}</span> : null}
            </header>
            <div className="shared-tree">
              {group.rows.map((row, index) => row.type === 'workspace' ? (
                <div key={`ws:${row.label}:${index}`} className="shared-tree-label"><Folder size={15} aria-hidden="true" /><span>{row.label}</span></div>
              ) : (
                <ShareRow key={row.item.key} item={row.item} depth={row.depth} showDevice={false} showPerson {...rowProps}
                  // Under its device's heading, a whole-device share is named by its scope.
                  title={row.item.scope === 'device' ? translate('sharing.kindDevice') : row.item.title} />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

function PeopleView({ items, direction, ...rowProps }: RowProps & { items: SharedItem[]; direction: ShareDirection }) {
  const people = useMemo(() => groupByPerson(items), [items]);
  const wide = useWide();
  const [selected, setSelected] = useState<string | null>(null);
  const current = people.find(person => person.personId === selected) ?? (wide ? people[0] : undefined);
  return (
    <div className="shared-people" data-narrow-open={!wide && current ? 'true' : undefined}>
      <nav className="shared-people-list" aria-label={translate(direction === 'in' ? 'sharing.peopleWhoShared' : 'sharing.peopleIShareWith')}>
        {people.map(person => (
          <button key={person.personId} type="button" className="shared-person" aria-current={current?.personId === person.personId ? 'true' : undefined} onClick={() => setSelected(person.personId)}>
            <Avatar name={person.personName} size="md" />
            <span className="shared-row-main">
              <span className="shared-row-title">{person.personName}</span>
              <span className="shared-row-sub"><span className="shared-sub-part">{personSummary(person)}</span></span>
            </span>
            {person.items.some(item => rowProps.attention.has(item.key)) ? <ShieldAlert className="shared-person-flag" size={15} role="img" aria-label={translate('sharing.filterAttention')} /> : null}
            <ChevronRight size={15} aria-hidden="true" className="shared-person-chevron" />
          </button>
        ))}
      </nav>
      {current ? (
        <section className="shared-person-pane" aria-label={current.personName}>
          <header className="shared-person-head">
            {!wide ? <button type="button" className="shared-back" aria-label={translate('sharing.backToPeople')} onClick={() => setSelected(null)}><ArrowLeft size={18} /></button> : null}
            <Avatar name={current.personName} size="lg" />
            <span className="shared-row-main">
              <span className="shared-person-name">{current.personName}</span>
              <span className="shared-row-sub">
                <span className="shared-sub-part">{translate(direction === 'in' ? 'sharing.sharedWithYouBy' : 'sharing.youShareWith')}</span>
                {direction === 'out' ? <span className="shared-sub-part">{current.lastAccessedAt ? translate('sharing.visited', { value1: formatDate(current.lastAccessedAt) }) : translate('sharing.noVisitsYet')}</span> : null}
              </span>
            </span>
          </header>
          {(['device', 'workspace', 'thread'] as const).map(scope => {
            const scoped = current.items.filter(item => item.scope === scope);
            if (!scoped.length) return null;
            const Icon = SCOPE_ICON[scope];
            return (
              <div key={scope} className="shared-person-section">
                <h3><Icon size={13} aria-hidden="true" />{translate(scope === 'device' ? 'sharing.kindDevice' : scope === 'workspace' ? 'sharing.filterWorkspaces' : 'sharing.filterThreads')}</h3>
                <div className="shared-list">{scoped.map(item => <ShareRow key={item.key} item={item} showPerson={false} {...rowProps} />)}</div>
              </div>
            );
          })}
        </section>
      ) : null}
    </div>
  );
}

function personSummary(person: PersonNode) {
  return [
    person.counts.device ? translate('sharing.countDevices', { count: person.counts.device }) : null,
    person.counts.workspace ? translate('sharing.countWorkspaces', { count: person.counts.workspace }) : null,
    person.counts.thread ? translate('sharing.countThreads', { count: person.counts.thread }) : null,
  ].filter(Boolean).join(' · ');
}
