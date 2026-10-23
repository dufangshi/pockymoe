// The relay admin console: a sidebar of sections on desktop (it folds into an
// icon rail), a bottom tab bar with a "More" sheet on phones, and one page per
// section. The URL holds the section (`tab`), its sub-tab (`sub`), the open row
// (`id`), the time range and each table's sort and column filters, so every
// view can be linked and survives a reload.
import { useMenuKeyboard, useModalFocus } from '@pockymoe/thread-ui';
import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  Activity,
  CalendarRange,
  CircleCheck,
  CircleAlert,
  HeartPulse,
  LayoutGrid,
  LogOut,
  Menu,
  MonitorSmartphone,
  PanelLeftClose,
  PanelLeftOpen,
  RadioTower,
  RefreshCw,
  Search,
  Server,
  Settings,
  Share2,
  ShieldCheck,
  Users,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useLocation, useNavigate } from 'react-router-dom';
import type { RelaySessionDto } from '@pockymoe/shared';
import { adminApi } from './adminApi';
import { formatCount, formatDuration, formatRelative, since } from './format';
import type { AdminOverview, AdminRange, AdminSearchResult } from './types';
import { Avatar, useDebounced, useIsPhone } from './ui';
import { rangeLabel } from './labels';
import { usePopover } from './table';
import { PerformancePage } from './PerformancePage';
import { OverviewPage } from './OverviewPage';
import { UsersPage } from './UsersPage';
import { DevicesPage } from './DevicesPage';
import { AccessPage } from './AccessPage';
import { SecurityPage } from './SecurityPage';
import { SystemPage } from './SystemPage';
import { ServerPage } from './ServerPage';
import { SettingsPage } from './SettingsPage';
import './relay-admin.css';

export type AdminSection = 'overview' | 'users' | 'devices' | 'performance' | 'server' | 'shares' | 'security' | 'system' | 'settings';
export const ADMIN_SECTIONS: AdminSection[] = ['overview', 'users', 'devices', 'performance', 'server', 'shares', 'security', 'system', 'settings'];
const PHONE_TABS: AdminSection[] = ['overview', 'users', 'devices', 'security'];
const MORE_SECTIONS: AdminSection[] = ['performance', 'server', 'shares', 'system', 'settings'];
/** Sections whose charts follow the time range (presets or a custom window). */
const RANGE_SECTIONS: AdminSection[] = ['overview', 'performance', 'server', 'shares', 'security', 'system'];
const RANGES: AdminRange[] = ['24h', '7d', '30d', '90d'];
const SIDEBAR_KEY = 'pockymoe-relay-admin-sidebar';

/** A custom time window (`range=custom&from=YYYY-MM-DD&to=YYYY-MM-DD`). */
export interface CustomRange {
  from: string;
  to: string;
}

export interface AdminPageProps {
  range: AdminRange;
  /** Set when the URL holds a custom window. */
  customRange: CustomRange | null;
  sub: string | null;
  filter: string | null;
  selectedId: string | null;
  /** Every URL value, for a table's sort and column filters. */
  params: URLSearchParams;
  refreshKey: number;
  go: (section: AdminSection, options?: { sub?: string | null; filter?: string | null; id?: string | null; params?: Record<string, string> }) => void;
  /** Writes URL values (null removes one); only opening or closing a row replaces history. */
  setParams: (params: Record<string, string | null>) => void;
  onLoaded: () => void;
  onUnauthorized: () => void;
  /** Reloads the counts in the navigation after an action changed them. */
  onChanged: () => void;
}

export { pageWindow } from './ranges';

export function sectionLabel(section: AdminSection) {
  switch (section) {
    case 'overview': return translate('admin.navOverview');
    case 'users': return translate('admin.navUsers');
    case 'devices': return translate('admin.navDevices');
    case 'performance': return translate('admin.navPerformance');
    case 'server': return translate('admin.navServer');
    case 'shares': return translate('admin.navShares');
    case 'security': return translate('admin.navSecurity');
    case 'system': return translate('admin.navSystem');
    case 'settings': return translate('admin.navSettings');
  }
}

function sectionIcon(section: AdminSection) {
  const props = { 'aria-hidden': true, className: 'h-[18px] w-[18px]' } as const;
  switch (section) {
    case 'overview': return <LayoutGrid {...props} />;
    case 'users': return <Users {...props} />;
    case 'devices': return <MonitorSmartphone {...props} />;
    case 'performance': return <Activity {...props} />;
    case 'server': return <HeartPulse {...props} />;
    case 'shares': return <Share2 {...props} />;
    case 'security': return <ShieldCheck {...props} />;
    case 'system': return <Server {...props} />;
    case 'settings': return <Settings {...props} />;
  }
}

function sectionFromSearch(search: string): AdminSection {
  const value = new URLSearchParams(search).get('tab');
  return ADMIN_SECTIONS.includes(value as AdminSection) ? (value as AdminSection) : 'overview';
}

export function RelayAdminConsole({ session, onLogout, onUnauthorized }: {
  session: RelaySessionDto | null;
  onLogout: () => void;
  onUnauthorized: () => void;
}) {
  useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const phone = useIsPhone();
  const params = useMemo(() => new URLSearchParams(location.search), [location.search]);
  const section = sectionFromSearch(location.search);
  const range = (RANGES.includes(params.get('range') as AdminRange) ? params.get('range') : '24h') as AdminRange;
  const customRange = useMemo<CustomRange | null>(() => {
    const day = /^\d{4}-\d{2}-\d{2}$/;
    const from = params.get('from') ?? '';
    const to = params.get('to') ?? '';
    return params.get('range') === 'custom' && day.test(from) && day.test(to) && from <= to ? { from, to } : null;
  }, [params]);
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem(SIDEBAR_KEY) === 'collapsed';
    } catch {
      return false;
    }
  });
  const toggleSidebar = () => {
    setCollapsed((value) => {
      try {
        localStorage.setItem(SIDEBAR_KEY, value ? 'expanded' : 'collapsed');
      } catch {
        /* The choice lasts this visit. */
      }
      return !value;
    });
  };
  const [refreshKey, setRefreshKey] = useState(0);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [nav, setNav] = useState<AdminOverview | null>(null);
  const [navFailed, setNavFailed] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [, setTick] = useState(0);

  const write = useCallback((next: URLSearchParams, replace = false) => {
    for (const [key, value] of [...next.entries()]) if (!value) next.delete(key);
    navigate(`${location.pathname}?${next.toString()}`, { replace });
  }, [location.pathname, navigate]);

  const go: AdminPageProps['go'] = useCallback((target, options = {}) => {
    const next = new URLSearchParams();
    next.set('tab', target);
    // The time range follows to every section that charts over time.
    const keep = params.get('range');
    if (keep && RANGE_SECTIONS.includes(target)) {
      next.set('range', keep);
      if (keep === 'custom') {
        for (const key of ['from', 'to']) if (params.get(key)) next.set(key, params.get(key)!);
      }
    }
    if (options.sub) next.set('sub', options.sub);
    if (options.filter) next.set('filter', options.filter);
    if (options.id) next.set('id', options.id);
    for (const [key, value] of Object.entries(options.params ?? {})) next.set(key, value);
    setMoreOpen(false);
    write(next);
  }, [params, write]);

  const setParams: AdminPageProps['setParams'] = useCallback((values) => {
    const next = new URLSearchParams(location.search);
    for (const [key, value] of Object.entries(values)) {
      if (value) next.set(key, value);
      else next.delete(key);
    }
    // Opening and closing details replace the entry: Back leaves the page.
    write(next, Object.keys(values).every((key) => key === 'id'));
  }, [location.search, write]);

  // The navigation counts and the status pill come from a light overview read.
  const loadNav = useCallback(() => {
    adminApi
      .overview({ range: '24h' })
      .then((overview) => {
        setNav(overview);
        setNavFailed(false);
      })
      .catch(() => setNavFailed(true));
  }, []);
  useEffect(() => {
    loadNav();
  }, [loadNav, refreshKey]);

  // "Updated 1 minute ago" stays current.
  useEffect(() => {
    const timer = window.setInterval(() => setTick((value) => value + 1), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  // `/` focuses the search, as in the design.
  const searchRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target && (target.closest('input, textarea, select, [contenteditable="true"]'))) return;
      event.preventDefault();
      searchRef.current?.focus();
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, []);

  const onLoaded = useCallback(() => setUpdatedAt(Date.now()), []);
  const pageProps: AdminPageProps = {
    range,
    customRange: RANGE_SECTIONS.includes(section) ? customRange : null,
    sub: params.get('sub'),
    filter: params.get('filter'),
    selectedId: params.get('id'),
    params,
    refreshKey,
    go,
    setParams,
    onLoaded,
    onUnauthorized,
    onChanged: loadNav,
  };

  const badge = (target: AdminSection): { value: number; attention?: boolean } | null => {
    if (!nav) return null;
    if (target === 'users') return nav.attention.pendingRegistrations ? { value: nav.attention.pendingRegistrations, attention: true } : null;
    if (target === 'devices') return { value: nav.devices.total };
    if (target === 'shares') return { value: nav.shares.active };
    return null;
  };

  const user = session?.user ?? null;
  const healthy = !navFailed;
  const host = hostOf(nav?.relay.publicBaseUrl ?? null);
  const showRange = RANGE_SECTIONS.includes(section);
  const allowCustom = showRange;
  const setRange = (value: AdminRange | CustomRange) => setParams(typeof value === 'string' ? { range: value, from: null, to: null } : { range: 'custom', from: value.from, to: value.to });
  const title = sectionLabel(section);

  const page = (() => {
    switch (section) {
      case 'overview': return <OverviewPage {...pageProps} />;
      case 'users': return <UsersPage {...pageProps} />;
      case 'devices': return <DevicesPage {...pageProps} />;
      case 'performance': return <PerformancePage {...pageProps} />;
      case 'server': return <ServerPage {...pageProps} />;
      case 'shares': return <AccessPage {...pageProps} />;
      case 'security': return <SecurityPage {...pageProps} />;
      case 'system': return <SystemPage {...pageProps} />;
      case 'settings': return <SettingsPage {...pageProps} session={session} />;
    }
  })();

  return (
    <div className={`admin-console${phone ? ' is-phone' : ''}${!phone && collapsed ? ' is-collapsed' : ''}`} data-page="admin">
      {!phone ? (
        <aside className="admin-sidebar">
          <div className="admin-sidebar-inner">
          <div className="admin-brand">
            <span aria-hidden className="admin-brand-mark"><RadioTower className="h-5 w-5" /></span>
            <span className="admin-brand-text">
              <b>{translate('admin.consoleTitle')}</b>
              <span title={host}>{host}</span>
            </span>
            <button
              aria-controls="admin-nav"
              aria-expanded={!collapsed}
              aria-label={collapsed ? translate('admin.expandSidebar') : translate('admin.collapseSidebar')}
              className="admin-icon-button admin-sidebar-toggle"
              data-tip={collapsed ? translate('admin.expandSidebar') : translate('admin.collapseSidebar')}
              onClick={toggleSidebar}
              type="button"
            >
              {collapsed ? <PanelLeftOpen aria-hidden className="h-[18px] w-[18px]" /> : <PanelLeftClose aria-hidden className="h-[18px] w-[18px]" />}
            </button>
          </div>
          <nav aria-label={translate('admin.sections')} className="admin-nav" id="admin-nav">
            {ADMIN_SECTIONS.map((item) => {
              const count = badge(item);
              return (
                <button
                  aria-current={section === item ? 'page' : undefined}
                  className="admin-nav-item"
                  // The rail shows icons only: the name appears beside it on
                  // hover and keyboard focus, and stays the accessible name.
                  data-tip={collapsed ? sectionLabel(item) : undefined}
                  key={item}
                  onClick={() => go(item)}
                  type="button"
                >
                  <span className="admin-nav-icon">
                    {sectionIcon(item)}
                    {collapsed && count?.attention ? <i aria-hidden className="is-attention" /> : null}
                  </span>
                  <span className="admin-nav-label">{sectionLabel(item)}</span>
                  {count ? (
                    <b className={`admin-nav-count${count.attention ? ' is-attention' : ''}`}>
                      {formatCount(count.value)}
                      {count.attention ? <span className="sr-only"> {translate('admin.pendingRegistrationsShort')}</span> : null}
                    </b>
                  ) : null}
                </button>
              );
            })}
          </nav>
          {user ? (
            <div className="admin-account">
              <Avatar name={user.username} />
              <span className="admin-account-text">
                <b>{user.username}</b>
                <span>{translate('admin.roleAdmin')}</span>
              </span>
              <button aria-label={translate('admin.logout')} className="admin-icon-button" data-tip={translate('admin.logout')} onClick={onLogout} type="button">
                <LogOut aria-hidden className="h-4 w-4" />
              </button>
            </div>
          ) : null}
          </div>
        </aside>
      ) : null}
      <div className="admin-main">
        <header className="admin-topbar">
          {phone ? (
            <span aria-hidden className="admin-brand-mark is-small"><RadioTower className="h-4 w-4" /></span>
          ) : null}
          <div className="admin-topbar-title">
            <h1 title={title}>{title}</h1>
            {!phone ? <p>{pageSubtitle(section, nav)}</p> : null}
          </div>
          <div className="admin-topbar-tools">
            <span className={`admin-health ${healthy ? 'is-ok' : 'is-down'}`} role="status" title={phone ? (healthy ? translate('admin.healthy') : translate('admin.unreachable')) : undefined}>
              {healthy ? <CircleCheck aria-hidden className="h-4 w-4" /> : <CircleAlert aria-hidden className="h-4 w-4" />}
              <span className={phone ? 'sr-only' : 'admin-health-text'}>{healthy ? translate('admin.healthy') : translate('admin.unreachable')}</span>
            </span>
            {showRange && !phone ? <RangeSwitch custom={allowCustom ? customRange : undefined} onChange={setRange} range={customRange && allowCustom ? null : range} /> : null}
            <GlobalSearch compact={phone} inputRef={searchRef} onOpen={(target, id) => go(target, { id })} />
            {!phone ? (
              <span className="admin-updated">
                {updatedAt ? <span className="admin-updated-text">{translate('admin.updatedAgo', { time: formatRelative(updatedAt) })}</span> : null}
                <button
                  aria-label={translate('admin.refresh')}
                  className="admin-icon-button"
                  onClick={() => setRefreshKey((value) => value + 1)}
                  title={translate('admin.refresh')}
                  type="button"
                >
                  <RefreshCw aria-hidden className="h-4 w-4" />
                </button>
              </span>
            ) : null}
          </div>
        </header>
        {showRange && phone ? (
          <div className="admin-phone-range"><RangeSwitch custom={allowCustom ? customRange : undefined} onChange={setRange} range={customRange && allowCustom ? null : range} /></div>
        ) : null}
        <main className="admin-content" id="admin-content">
          {page}
        </main>
      </div>
      {phone ? (
        <>
          <nav aria-label={translate('admin.sections')} className="admin-tabbar">
            {PHONE_TABS.map((item) => {
              const count = badge(item);
              return (
                <button aria-current={section === item ? 'page' : undefined} key={item} onClick={() => go(item)} type="button">
                  <span className="admin-tabbar-icon">
                    {sectionIcon(item)}
                    {count?.attention ? <b>{formatCount(count.value)}</b> : null}
                  </span>
                  <span>{item === 'security' ? translate('admin.navSecurityShort') : sectionLabel(item)}</span>
                </button>
              );
            })}
            <button
              aria-current={!PHONE_TABS.includes(section) ? 'page' : undefined}
              aria-expanded={moreOpen}
              aria-haspopup="dialog"
              onClick={() => setMoreOpen(true)}
              type="button"
            >
              <span className="admin-tabbar-icon"><Menu aria-hidden className="h-[18px] w-[18px]" /></span>
              <span>{translate('admin.more')}</span>
            </button>
          </nav>
          <MoreSheet
            badge={badge}
            current={section}
            onClose={() => setMoreOpen(false)}
            onGo={(item) => go(item)}
            onLogout={onLogout}
            onRefresh={() => {
              setMoreOpen(false);
              setRefreshKey((value) => value + 1);
            }}
            open={moreOpen}
          />
        </>
      ) : null}
    </div>
  );
}

function hostOf(url: string | null) {
  try {
    return url ? new URL(url).host : window.location.host;
  } catch {
    return window.location.host;
  }
}

function pageSubtitle(section: AdminSection, nav: AdminOverview | null): ReactNode {
  if (!nav) return null;
  switch (section) {
    case 'overview':
      return translate('admin.overviewSubtitle', { host: hostOf(nav.relay.publicBaseUrl), version: nav.relay.version, uptime: formatDuration(since(nav.relay.startedAt)) });
    case 'users':
      return translate('admin.usersSubtitle', { total: formatCount(nav.users.total), enabled: formatCount(nav.users.enabled), disabled: formatCount(nav.users.disabled) });
    case 'devices':
      return translate('admin.devicesSubtitle', { total: formatCount(nav.devices.total), online: formatCount(nav.devices.online), protected: formatCount(nav.devices.protected) });
    case 'performance':
      return translate('admin.performanceSubtitle');
    case 'server':
      return translate('admin.serverSubtitle');
    case 'shares':
      return translate('admin.sharesSubtitle', { shares: formatCount(nav.shares.active), links: formatCount(nav.publicLinks.total) });
    case 'security':
      return translate('admin.securitySubtitle');
    case 'system':
      return translate('admin.systemSubtitle');
    case 'settings':
      return translate('admin.settingsSubtitle');
  }
}

/**
 * The time range: four presets, and on pages that take one a custom window
 * (`custom` is the current one or null; undefined hides the button).
 */
export function RangeSwitch({ range, onChange, custom }: {
  range: AdminRange | null;
  onChange: (range: AdminRange | CustomRange) => void;
  custom?: CustomRange | null | undefined;
}) {
  useI18n();
  return (
    <div aria-label={translate('admin.timeRange')} className="admin-range" role="group">
      {RANGES.map((value) => (
        <button aria-pressed={range === value} key={value} onClick={() => onChange(value)} type="button">
          {rangeLabel(value)}
        </button>
      ))}
      {custom !== undefined ? <CustomRangeButton current={custom} onChange={onChange} /> : null}
    </div>
  );
}

function shortDate(day: string) {
  const [year, month, date] = day.split('-').map(Number);
  return new Intl.DateTimeFormat(getLocale(), { month: 'short', day: 'numeric' }).format(new Date(year!, (month ?? 1) - 1, date ?? 1));
}

function localDay(offsetDays = 0) {
  const now = new Date(Date.now() - offsetDays * 86_400_000);
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
}

function CustomRangeButton({ current, onChange }: { current: CustomRange | null; onChange: (range: CustomRange) => void }) {
  useI18n();
  const popover = usePopover();
  const [from, setFrom] = useState(current?.from ?? localDay(7));
  const [to, setTo] = useState(current?.to ?? localDay());
  useEffect(() => {
    if (!popover.open) return;
    setFrom(current?.from ?? localDay(7));
    setTo(current?.to ?? localDay());
  }, [popover.open, current]);
  const invalid = !from || !to || from > to;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (invalid) return;
    onChange({ from, to });
    popover.close(true);
  };
  return (
    <>
      <button
        aria-expanded={popover.open}
        aria-haspopup="dialog"
        aria-pressed={Boolean(current)}
        onClick={() => popover.setOpen((value) => !value)}
        ref={popover.trigger}
        type="button"
      >
        <CalendarRange aria-hidden className="h-4 w-4" />
        <span>{current ? `${shortDate(current.from)} – ${shortDate(current.to)}` : translate('admin.customRangeShort')}</span>
      </button>
      {popover.open
        ? createPortal(
            <div
              aria-label={translate('admin.customRange')}
              className="admin-menu admin-filter-menu admin-range-form"
              onKeyDown={(event) => {
                if (event.key === 'Escape') {
                  event.stopPropagation();
                  popover.close(true);
                }
              }}
              ref={popover.panel}
              role="dialog"
              style={popover.place ? { top: popover.place.top, left: popover.place.left, maxHeight: popover.place.maxHeight } : { top: -9999, left: -9999 }}
            >
              <form onSubmit={submit}>
                <label>
                  <span>{translate('admin.fromDay')}</span>
                  <input autoFocus className="relay-input" max={to || localDay()} onChange={(event) => setFrom(event.target.value)} type="date" value={from} />
                </label>
                <label>
                  <span>{translate('admin.toDay')}</span>
                  <input className="relay-input" max={localDay()} min={from || undefined} onChange={(event) => setTo(event.target.value)} type="date" value={to} />
                </label>
                {from && to && from > to ? <p className="admin-field-hint">{translate('admin.rangeOrder')}</p> : null}
                <div className="admin-form-actions">
                  <button className="relay-button-secondary" onClick={() => popover.close(true)} type="button">{translate('admin.cancel')}</button>
                  <button className="relay-button-primary" disabled={invalid} type="submit">{translate('admin.apply')}</button>
                </div>
              </form>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/** Users and devices by name, email or ID; opens the row's details. */
function GlobalSearch({ compact, inputRef, onOpen }: {
  compact: boolean;
  inputRef: React.RefObject<HTMLInputElement | null>;
  onOpen: (section: AdminSection, id: string) => void;
}) {
  useI18n();
  const [text, setText] = useState('');
  const [expanded, setExpanded] = useState(!compact);
  const [results, setResults] = useState<AdminSearchResult | null>(null);
  const [open, setOpen] = useState(false);
  const query = useDebounced(text.trim(), 200);
  const menu = useRef<HTMLDivElement>(null);
  const wrapper = useRef<HTMLDivElement>(null);
  const close = useCallback((restore: boolean) => {
    setOpen(false);
    if (restore) inputRef.current?.focus();
  }, [inputRef]);
  useMenuKeyboard(menu, open && Boolean(results && (results.users.length || results.devices.length)) && document.activeElement !== inputRef.current, close);
  useEffect(() => {
    if (query.length < 2) {
      setResults(null);
      return;
    }
    let cancelled = false;
    adminApi.search(query).then((value) => {
      if (!cancelled) {
        setResults(value);
        setOpen(true);
      }
    }).catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [query]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!wrapper.current?.contains(event.target as Node) && !menu.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener('pointerdown', outside, true);
    return () => window.removeEventListener('pointerdown', outside, true);
  }, [open]);
  const pick = (section: AdminSection, id: string) => {
    setOpen(false);
    setText('');
    if (compact) setExpanded(false);
    onOpen(section, id);
  };
  const rect = wrapper.current?.getBoundingClientRect();
  if (compact && !expanded) {
    return (
      <button aria-label={translate('admin.search')} className="admin-icon-button" onClick={() => {
        setExpanded(true);
        requestAnimationFrame(() => inputRef.current?.focus());
      }} type="button">
        <Search aria-hidden className="h-5 w-5" />
      </button>
    );
  }
  const empty = results && !results.users.length && !results.devices.length;
  return (
    <div className={`admin-global-search${compact ? ' is-compact' : ''}`} ref={wrapper}>
      <Search aria-hidden className="h-4 w-4" />
      <input
        aria-label={translate('admin.search')}
        onBlur={() => {
          if (compact && !text) setExpanded(false);
        }}
        onChange={(event) => setText(event.target.value)}
        onFocus={() => results && setOpen(true)}
        onKeyDown={(event) => {
          if (event.key === 'ArrowDown' && open) {
            event.preventDefault();
            menu.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
          } else if (event.key === 'Escape') {
            setOpen(false);
          }
        }}
        placeholder={translate('admin.searchPlaceholder')}
        ref={inputRef}
        type="search"
        value={text}
      />
      {!compact ? <kbd aria-hidden>/</kbd> : null}
      {compact ? (
        <button aria-label={translate('admin.close')} className="admin-icon-button" onClick={() => {
          setText('');
          setExpanded(false);
        }} type="button">
          <X aria-hidden className="h-4 w-4" />
        </button>
      ) : null}
      {open && results && rect
        ? createPortal(
            <div aria-label={translate('admin.searchResults')} className="admin-menu admin-search-results" ref={menu} role="menu" style={{ top: rect.bottom + 4, left: Math.max(8, Math.min(rect.left, window.innerWidth - Math.max(rect.width, 280) - 8)), width: Math.max(rect.width, 280) }}>
              {empty ? <p className="admin-search-empty">{translate('admin.noMatches')}</p> : null}
              {results.users.map((user) => (
                <button key={`u-${user.id}`} onClick={() => pick('users', user.id)} role="menuitem" tabIndex={-1} type="button">
                  <Users aria-hidden className="h-4 w-4" />
                  <span>{user.username}</span>
                  <small>{user.email}</small>
                </button>
              ))}
              {results.devices.map((device) => (
                <button key={`d-${device.id}`} onClick={() => pick('devices', device.id)} role="menuitem" tabIndex={-1} type="button">
                  <MonitorSmartphone aria-hidden className="h-4 w-4" />
                  <span>{device.name}</span>
                  <small>{device.ownerUsername}</small>
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </div>
  );
}

/** Phones: the sections that do not fit the tab bar, rising from the bottom. */
function MoreSheet({ open, onClose, onGo, onLogout, onRefresh, current, badge }: {
  open: boolean;
  onClose: () => void;
  onGo: (section: AdminSection) => void;
  onLogout: () => void;
  onRefresh: () => void;
  current: AdminSection;
  badge: (section: AdminSection) => { value: number } | null;
}) {
  useI18n();
  const ref = useRef<HTMLDivElement>(null);
  useModalFocus(ref, open, onClose);
  if (!open) return null;
  return createPortal(
    <div className="admin-detail-layer is-sheet">
      <button aria-label={translate('admin.close')} className="admin-detail-scrim" onClick={onClose} tabIndex={-1} type="button" />
      <div aria-label={translate('admin.more')} aria-modal="true" className="admin-detail admin-more-sheet" ref={ref} role="dialog">
        <span aria-hidden className="admin-sheet-grip" />
        <header className="admin-detail-header">
          <div className="admin-detail-titles"><h2>{translate('admin.more')}</h2></div>
          <button aria-label={translate('admin.close')} className="admin-icon-button admin-detail-close" onClick={onClose} type="button">
            <X aria-hidden data-icon="close" className="h-5 w-5" />
          </button>
        </header>
        <div className="admin-more-list">
          {MORE_SECTIONS.map((item) => {
            const count = badge(item);
            return (
              <button aria-current={current === item ? 'page' : undefined} key={item} onClick={() => onGo(item)} type="button">
                {sectionIcon(item)}
                <span>{sectionLabel(item)}</span>
                {count ? <b>{formatCount(count.value)}</b> : null}
              </button>
            );
          })}
          <button onClick={onRefresh} type="button">
            <RefreshCw aria-hidden className="h-[18px] w-[18px]" />
            <span>{translate('admin.refresh')}</span>
          </button>
          <button className="is-danger" onClick={onLogout} type="button">
            <LogOut aria-hidden className="h-[18px] w-[18px]" />
            <span>{translate('admin.logout')}</span>
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
