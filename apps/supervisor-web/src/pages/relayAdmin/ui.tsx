// Building blocks of the admin console: cards, figures, filters, tables with
// paging, the detail window (right drawer on desktop, bottom sheet on phones),
// row menus and the device connection marks (docs/ui-principles.md).
import { ErrorMark, useMenuKeyboard, useModalFocus } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ChevronLeft, ChevronRight, MoreHorizontal, Search, ShieldAlert, ShieldCheck, X } from 'lucide-react';
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { ApiError } from '../../lib/api';
import { useMediaQuery } from './charts';
import { formatCount, initials } from './format';

export const PHONE_QUERY = '(max-width: 767px)';

export function useIsPhone() {
  return useMediaQuery(PHONE_QUERY);
}

export function errorText(caught: unknown) {
  if (caught instanceof ApiError) return caught.payload.message || `HTTP ${caught.statusCode}`;
  if (caught instanceof Error) return caught.message;
  return translate('admin.requestFailed');
}

/**
 * Loads data for a page and reloads it when `key` changes. The previous data
 * stays on screen while a reload runs (no flash of empty content); a failed
 * background reload keeps it too and only sets `error`.
 */
export function useAdminResource<T>(load: () => Promise<T>, key: string, onUnauthorized?: () => void) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [version, setVersion] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const unauthorized = useRef(onUnauthorized);
  unauthorized.current = onUnauthorized;
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    loadRef.current()
      .then((value) => {
        if (cancelled) return;
        setData(value);
        setError(null);
      })
      .catch((caught: unknown) => {
        if (cancelled) return;
        if (caught instanceof ApiError && (caught.statusCode === 401 || caught.statusCode === 403)) unauthorized.current?.();
        setError(errorText(caught));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [key, version]);
  const reload = useCallback(() => setVersion((value) => value + 1), []);
  return { data, error, loading, reload, setData };
}

export function Card({
  title,
  subtitle,
  aside,
  children,
  className,
  labelledBy,
}: {
  title?: string;
  subtitle?: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
  labelledBy?: string;
}) {
  const id = useId();
  return (
    <section aria-labelledby={title ? id : labelledBy} className={`admin-card${className ? ` ${className}` : ''}`}>
      {title ? (
        <header className="admin-card-header">
          <div className="admin-card-heading">
            <h3 id={id}>{title}</h3>
            {subtitle ? <p>{subtitle}</p> : null}
          </div>
          {aside ? <div className="admin-card-aside">{aside}</div> : null}
        </header>
      ) : null}
      {children}
    </section>
  );
}

/** A key figure: label, value (with an optional "/ total"), one detail line. */
export function Figure({ icon, label, value, total, detail, trend }: {
  icon?: ReactNode;
  label: string;
  value: ReactNode;
  total?: ReactNode;
  detail?: ReactNode;
  trend?: ReactNode;
}) {
  return (
    <div className="admin-figure">
      <dt>
        {icon ? <span aria-hidden className="admin-figure-icon">{icon}</span> : null}
        {label}
      </dt>
      <dd>
        <span className="admin-figure-value">{value}</span>
        {total !== undefined ? <span className="admin-figure-total">/ {total}</span> : null}
        {trend ? <span className="admin-figure-trend">{trend}</span> : null}
      </dd>
      {detail ? <dd className="admin-figure-detail">{detail}</dd> : null}
    </div>
  );
}

export function FigureBand({ children, label }: { children: ReactNode; label: string }) {
  return (
    <section aria-label={label} className="admin-card admin-figure-band">
      <dl>{children}</dl>
    </section>
  );
}

export interface ChipOption<K extends string> {
  key: K;
  label: string;
  count?: number | null | undefined;
  /** Warning chips draw attention (unprotected, no account key). */
  tone?: 'warning' | undefined;
  icon?: ReactNode | undefined;
}

/** One-of-many filter shown as chips with counts. */
export function FilterChips<K extends string>({ options, value, onChange, label }: {
  options: Array<ChipOption<K>>;
  value: K;
  onChange: (key: K) => void;
  label: string;
}) {
  return (
    <div aria-label={label} className="admin-chips" role="group">
      {options.map((option) => (
        <button
          aria-pressed={value === option.key}
          className={`admin-chip${option.tone ? ` is-${option.tone}` : ''}`}
          key={option.key}
          onClick={() => onChange(option.key)}
          type="button"
        >
          {option.icon}
          <span>{option.label}</span>
          {option.count !== undefined && option.count !== null ? <b>{formatCount(option.count)}</b> : null}
        </button>
      ))}
    </div>
  );
}

/** Tabs inside a page: ←/→/Home/End move between them. */
export function SubTabs<K extends string>({ tabs, value, onChange, label, idPrefix }: {
  /** `attention` marks a count of things to act on (red), with its accessible name. */
  tabs: Array<{ key: K; label: string; count?: number | null; icon?: ReactNode; attention?: string | undefined }>;
  value: K;
  onChange: (key: K) => void;
  label: string;
  idPrefix: string;
}) {
  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    const last = tabs.length - 1;
    const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index - 1 + tabs.length) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? last : null;
    if (next === null) return;
    event.preventDefault();
    const tab = tabs[next];
    if (!tab) return;
    onChange(tab.key);
    requestAnimationFrame(() => document.getElementById(`${idPrefix}-tab-${tab.key}`)?.focus());
  };
  return (
    <div aria-label={label} className="admin-subtabs" role="tablist">
      {tabs.map((tab, index) => (
        <button
          aria-controls={`${idPrefix}-panel`}
          aria-selected={value === tab.key}
          className="admin-subtab"
          id={`${idPrefix}-tab-${tab.key}`}
          key={tab.key}
          onClick={() => onChange(tab.key)}
          onKeyDown={(event) => onKeyDown(event, index)}
          role="tab"
          tabIndex={value === tab.key ? 0 : -1}
          type="button"
        >
          {tab.icon}
          <span>{tab.label}</span>
          {tab.count !== undefined && tab.count !== null ? (
            <b className={tab.attention ? 'is-attention' : undefined}>
              {formatCount(tab.count)}
              {tab.attention ? <span className="sr-only"> {tab.attention}</span> : null}
            </b>
          ) : null}
        </button>
      ))}
    </div>
  );
}

export function SearchField({ value, onChange, placeholder, label }: { value: string; onChange: (value: string) => void; placeholder: string; label: string }) {
  return (
    <label className="admin-search">
      <Search aria-hidden className="h-4 w-4" />
      <span className="sr-only">{label}</span>
      <input onChange={(event) => onChange(event.target.value)} placeholder={placeholder} type="search" value={value} />
    </label>
  );
}

/** Debounces a value (search text) before it is sent. */
export function useDebounced<T>(value: T, delay = 250) {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delay);
    return () => window.clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}

export function Pagination({ page, pageSize, total, onPage, unit }: { page: number; pageSize: number; total: number; onPage: (page: number) => void; unit?: string }) {
  useI18n();
  const last = Math.max(1, Math.ceil(total / pageSize));
  const from = total ? (page - 1) * pageSize + 1 : 0;
  const to = Math.min(total, page * pageSize);
  return (
    <nav aria-label={translate('admin.pagination')} className="admin-pagination">
      <span>{translate('admin.pageRange', { from: formatCount(from), to: formatCount(to), total: formatCount(total) })}{unit ? ` ${unit}` : ''}</span>
      {last > 1 ? (
        <span className="admin-pagination-buttons">
          <button aria-label={translate('admin.previousPage')} className="admin-icon-button" disabled={page <= 1} onClick={() => onPage(page - 1)} type="button">
            <ChevronLeft aria-hidden className="h-4 w-4" />
          </button>
          <span aria-current="page">{translate('admin.pageOf', { page, pages: last })}</span>
          <button aria-label={translate('admin.nextPage')} className="admin-icon-button" disabled={page >= last} onClick={() => onPage(page + 1)} type="button">
            <ChevronRight aria-hidden className="h-4 w-4" />
          </button>
        </span>
      ) : null}
    </nav>
  );
}

/** The name in a row's first column: the button that opens its details. */
export function RowOpener({ onOpen, children, label }: { onOpen?: () => void; children: ReactNode; label: string }) {
  if (!onOpen) return <span className="admin-row-name">{children}</span>;
  return (
    <button aria-label={label} className="admin-row-name admin-row-opener" onClick={onOpen} type="button">
      {children}
    </button>
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <p className="admin-empty">{children}</p>;
}

export function Avatar({ name, size = 'md' }: { name: string; size?: 'sm' | 'md' | 'lg' }) {
  return <span aria-hidden className={`admin-avatar is-${size}`}>{initials(name)}</span>;
}

export function Badge({ tone = 'neutral', children, icon }: { tone?: 'neutral' | 'success' | 'warning' | 'danger' | 'info'; children: ReactNode; icon?: ReactNode }) {
  return (
    <span className={`admin-badge is-${tone}`}>
      {icon}
      {children}
    </span>
  );
}

export type DeviceState = 'online' | 'offline' | 'unstable';

/**
 * Device connection state, recognizable by shape alone (docs/ui-principles.md
 * "Device connection state"): solid dot online, hollow circle offline, diamond
 * reconnecting often. Always with its words or an accessible name.
 */
export function DeviceStateMark({ state, label, showLabel = true }: { state: DeviceState; label?: string | undefined; showLabel?: boolean }) {
  useI18n();
  const text = label ?? deviceStateLabel(state);
  return (
    <span className={`admin-device-state is-${state}`}>
      <span aria-hidden className="admin-device-state-mark" />
      {showLabel ? <span>{text}</span> : <span className="sr-only">{text}</span>}
    </span>
  );
}

export function deviceStateLabel(state: DeviceState) {
  return state === 'online' ? translate('admin.online') : state === 'unstable' ? translate('admin.unstable') : translate('admin.offline');
}

export function ProtectionMark({ protectedDevice, showLabel = true }: { protectedDevice: boolean; showLabel?: boolean }) {
  useI18n();
  const text = protectedDevice ? translate('admin.protected') : translate('admin.unprotected');
  return (
    <span className={`admin-protection ${protectedDevice ? 'is-protected' : 'is-unprotected'}`} title={showLabel ? undefined : text}>
      {protectedDevice ? <ShieldCheck aria-hidden className="h-4 w-4" /> : <ShieldAlert aria-hidden className="h-4 w-4" />}
      {showLabel ? <span>{text}</span> : <span className="sr-only">{text}</span>}
    </span>
  );
}

export interface MenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

/** The ⋯ menu of a row or window, rendered at the page root. */
export function RowMenu({ items, label }: { items: MenuItem[]; label: string }) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ top: number; left: number } | null>(null);
  const close = useCallback((restore: boolean) => {
    setOpen(false);
    if (restore) trigger.current?.focus();
  }, []);
  useMenuKeyboard(menu, open, close);
  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const button = trigger.current?.getBoundingClientRect();
      const box = menu.current?.getBoundingClientRect();
      if (!button) return;
      const width = box?.width ?? 200;
      const height = box?.height ?? 120;
      const below = button.bottom + 4 + height <= window.innerHeight - 8;
      setPlace({
        top: below ? button.bottom + 4 : Math.max(8, button.top - 4 - height),
        left: Math.max(8, Math.min(button.right - width, window.innerWidth - width - 8)),
      });
    };
    place();
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!menu.current?.contains(target) && !trigger.current?.contains(target)) setOpen(false);
    };
    // A menu anchored to a row closes when the page scrolls under it.
    const scroll = (event: Event) => {
      if (!menu.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener('pointerdown', outside, true);
    window.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', place);
    return () => {
      window.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', place);
    };
  }, [open]);
  return (
    <>
      <button
        aria-expanded={open}
        aria-haspopup="menu"
        aria-label={label}
        className="admin-icon-button admin-row-menu-trigger"
        onClick={(event) => {
          event.stopPropagation();
          setOpen((value) => !value);
        }}
        ref={trigger}
        type="button"
      >
        <MoreHorizontal aria-hidden className="h-4 w-4" />
      </button>
      {open
        ? createPortal(
            <div aria-label={label} className="admin-menu" ref={menu} role="menu" style={place ?? { top: -9999, left: -9999 }}>
              {items.map((item) => (
                <button
                  className={item.danger ? 'is-danger' : undefined}
                  disabled={item.disabled}
                  key={item.key}
                  onClick={() => {
                    setOpen(false);
                    item.onSelect();
                  }}
                  role="menuitem"
                  tabIndex={-1}
                  type="button"
                >
                  {item.icon}
                  <span>{item.label}</span>
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/**
 * The details of a row: a drawer on the right on desktop, a sheet from the
 * bottom on phones. Modal: focus moves in, Tab stays inside, Esc closes the
 * top window, focus returns to the row.
 */
export function DetailWindow({ open, title, subtitle, badges, onClose, children, actions, icon }: {
  open: boolean;
  title: string;
  subtitle?: ReactNode;
  badges?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  actions?: ReactNode;
  icon?: ReactNode;
}) {
  useI18n();
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const phone = useIsPhone();
  // Focus starts on the title, so the window opens without a ring on Close.
  useModalFocus(ref, open, onClose, '.admin-detail-titles h2');
  useEffect(() => {
    if (!open) return;
    const previous = document.body.style.overflow;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = previous;
    };
  }, [open]);
  if (!open) return null;
  return createPortal(
    <div className={`admin-detail-layer ${phone ? 'is-sheet' : 'is-drawer'}`}>
      <button aria-label={translate('admin.close')} className="admin-detail-scrim" onClick={onClose} tabIndex={-1} type="button" />
      <div aria-labelledby={titleId} aria-modal="true" className="admin-detail" ref={ref} role="dialog">
        {phone ? <span aria-hidden className="admin-sheet-grip" /> : null}
        <header className="admin-detail-header">
          {icon ? <span className="admin-detail-icon">{icon}</span> : null}
          <div className="admin-detail-titles">
            <h2 id={titleId} tabIndex={-1}>{title}</h2>
            {subtitle ? <p>{subtitle}</p> : null}
            {badges ? <div className="admin-detail-badges">{badges}</div> : null}
          </div>
          <button aria-label={translate('admin.close')} className="admin-icon-button admin-detail-close" onClick={onClose} type="button">
            <X aria-hidden data-icon="close" className="h-5 w-5" />
          </button>
        </header>
        <div className="admin-detail-body">{children}</div>
        {actions ? <footer className="admin-detail-actions">{actions}</footer> : null}
      </div>
    </div>,
    document.body,
  );
}

export function DetailSection({ title, icon, children }: { title: string; icon?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="admin-detail-section">
      <h3 id={id}>
        {icon}
        {title}
      </h3>
      {children}
    </section>
  );
}

export function Facts({ items }: { items: Array<{ label: string; value: ReactNode } | null | false> }) {
  return (
    <dl className="admin-facts">
      {items.filter(Boolean).map((item) => {
        const fact = item as { label: string; value: ReactNode };
        return (
          <div key={fact.label}>
            <dt>{fact.label}</dt>
            <dd>{fact.value}</dd>
          </div>
        );
      })}
    </dl>
  );
}

/** A load failure next to its section, with Retry (docs/ui-principles.md §11). */
export function LoadFailure({ message, onRetry }: { message: string; onRetry: () => void }) {
  useI18n();
  return (
    <p className="admin-load-failure">
      {translate('admin.couldNotLoad')}
      <ErrorMark message={message} onRetry={onRetry} />
    </p>
  );
}

export function Skeleton({ lines = 3 }: { lines?: number }) {
  return (
    <div aria-busy="true" className="admin-skeleton">
      {Array.from({ length: lines }, (_, index) => (
        <span key={index} style={{ width: `${90 - index * 15}%` }} />
      ))}
    </div>
  );
}

/** A labelled pair of numbers for small figure tiles in drawers. */
export function MiniStat({ value, label }: { value: ReactNode; label: string }) {
  return (
    <div className="admin-mini-stat">
      <b>{value}</b>
      <span>{label}</span>
    </div>
  );
}
