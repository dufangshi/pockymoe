// Column filters above a table, as one row of pills: "Status: Online ▾". A pill
// opens a menu of choices (with counts where the relay gives them), a person
// picker that searches, or a time range with presets and a custom range. The
// values live in the URL, so a filtered view can be linked and survives a
// reload; "Clear filters" removes them all.
import { useMenuKeyboard } from '@pockymoe/thread-ui';
import { getLocale, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Check, ChevronDown, Search, X } from 'lucide-react';
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { adminApi } from './adminApi';
import { formatCount } from './format';
import { usePopover } from './table';
import type { AdminSearchResult } from './types';
import { useDebounced } from './ui';

export function FilterBar({ children, active, onClear, label }: { children: ReactNode; active: number; onClear: () => void; label: string }) {
  useI18n();
  return (
    <div aria-label={label} className="admin-filter-bar" role="group">
      {children}
      {active ? (
        <button className="admin-text-button admin-filter-clear" onClick={onClear} type="button">
          <X aria-hidden className="h-3.5 w-3.5" />
          {translate('admin.clearFilters')}
        </button>
      ) : null}
    </div>
  );
}

function PillButton({ label, valueLabel, open, onClick, buttonRef, onClear }: {
  label: string;
  valueLabel: string | null;
  open: boolean;
  onClick: () => void;
  buttonRef: React.RefObject<HTMLButtonElement | null>;
  onClear?: (() => void) | undefined;
}) {
  useI18n();
  return (
    <span className={`admin-filter-pill${valueLabel ? ' is-set' : ''}`}>
      <button aria-expanded={open} aria-haspopup="true" onClick={onClick} ref={buttonRef} type="button">
        <span className="admin-filter-name">{label}</span>
        {valueLabel ? <span className="admin-filter-value">{valueLabel}</span> : null}
        <ChevronDown aria-hidden className="h-3.5 w-3.5" />
      </button>
      {valueLabel && onClear ? (
        <button aria-label={translate('admin.clearFilter', { filter: label })} className="admin-filter-x" onClick={onClear} type="button">
          <X aria-hidden className="h-3.5 w-3.5" />
        </button>
      ) : null}
    </span>
  );
}

function panelStyle(place: { top: number; left: number; maxHeight: number } | null) {
  return place ? { top: place.top, left: place.left, maxHeight: place.maxHeight } : { top: -9999, left: -9999 };
}

export interface SelectOption {
  value: string;
  label: string;
  count?: number | null | undefined;
  icon?: ReactNode;
}

/** One of several values ("All" clears it). */
export function SelectFilter({ label, value, options, onChange }: {
  label: string;
  value: string | null;
  options: SelectOption[];
  onChange: (value: string | null) => void;
}) {
  useI18n();
  const popover = usePopover();
  useMenuKeyboard(popover.panel, popover.open, popover.close, '[role^="menuitem"]:not([disabled])');
  const current = options.find((option) => option.value === value) ?? null;
  const pick = (next: string | null) => {
    onChange(next);
    popover.close(true);
  };
  return (
    <>
      <PillButton buttonRef={popover.trigger} label={label} onClear={() => onChange(null)} onClick={() => popover.setOpen((open) => !open)} open={popover.open} valueLabel={current?.label ?? (value ? value : null)} />
      {popover.open
        ? createPortal(
            <div aria-label={label} className="admin-menu admin-filter-menu" ref={popover.panel} role="menu" style={panelStyle(popover.place)}>
              <button aria-checked={!value} onClick={() => pick(null)} role="menuitemradio" tabIndex={-1} type="button">
                <span aria-hidden className="admin-check-box">{!value ? <Check className="h-3.5 w-3.5" /> : null}</span>
                <span>{translate('admin.filterAll')}</span>
              </button>
              {options.map((option) => (
                <button aria-checked={option.value === value} key={option.value} onClick={() => pick(option.value)} role="menuitemradio" tabIndex={-1} type="button">
                  <span aria-hidden className="admin-check-box">{option.value === value ? <Check className="h-3.5 w-3.5" /> : null}</span>
                  {option.icon}
                  <span className="admin-menu-text">{option.label}</span>
                  {option.count !== undefined && option.count !== null ? <b className="admin-menu-count">{formatCount(option.count)}</b> : null}
                </button>
              ))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/** A person, found by name or email. */
export function UserFilter({ label, value, valueLabel, onChange }: {
  label: string;
  value: string | null;
  /** The person's name, when the relay sent it with the list. */
  valueLabel: string | null | undefined;
  onChange: (value: string | null, name: string | null) => void;
}) {
  useI18n();
  const popover = usePopover();
  const [text, setText] = useState('');
  const query = useDebounced(text.trim(), 200);
  const [results, setResults] = useState<AdminSearchResult['users'] | null>(null);
  const [remembered, setRemembered] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const listId = useId();
  useEffect(() => {
    if (!popover.open) {
      setText('');
      setResults(null);
      return;
    }
    requestAnimationFrame(() => input.current?.focus());
  }, [popover.open]);
  useEffect(() => {
    if (!popover.open || query.length < 1) {
      setResults(null);
      return;
    }
    let cancelled = false;
    adminApi.search(query).then((value) => {
      if (!cancelled) setResults(value.users);
    }).catch(() => {
      if (!cancelled) setResults([]);
    });
    return () => {
      cancelled = true;
    };
  }, [query, popover.open]);
  const pick = (id: string | null, name: string | null) => {
    setRemembered(name);
    onChange(id, name);
    popover.close(true);
  };
  const items = () => Array.from(list.current?.querySelectorAll<HTMLElement>('[role="option"]') ?? []);
  const onListKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const all = items();
    const index = all.indexOf(document.activeElement as HTMLElement);
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      all[Math.min(all.length - 1, index + 1)]?.focus();
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      if (index <= 0) input.current?.focus();
      else all[index - 1]?.focus();
    }
  };
  const shown = value ? valueLabel ?? remembered ?? value.slice(0, 8) : null;
  return (
    <>
      <PillButton buttonRef={popover.trigger} label={label} onClear={() => pick(null, null)} onClick={() => popover.setOpen((open) => !open)} open={popover.open} valueLabel={shown} />
      {popover.open
        ? createPortal(
            <div aria-label={label} className="admin-menu admin-filter-menu admin-user-filter" ref={popover.panel} role="dialog" style={panelStyle(popover.place)}>
              <label className="admin-search is-menu">
                <Search aria-hidden className="h-4 w-4" />
                <span className="sr-only">{translate('admin.findPerson')}</span>
                <input
                  aria-controls={listId}
                  onChange={(event) => setText(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'ArrowDown') {
                      event.preventDefault();
                      items()[0]?.focus();
                    }
                  }}
                  placeholder={translate('admin.findPerson')}
                  ref={input}
                  type="search"
                  value={text}
                />
              </label>
              <div aria-label={label} className="admin-user-filter-list" id={listId} onKeyDown={onListKey} ref={list} role="listbox">
                {value ? (
                  <button aria-selected={false} className="admin-menu-option" onClick={() => pick(null, null)} role="option" type="button">
                    <X aria-hidden className="h-4 w-4" />
                    <span>{translate('admin.anyone')}</span>
                  </button>
                ) : null}
                {(results ?? []).map((user) => (
                  <button aria-selected={user.id === value} className="admin-menu-option" key={user.id} onClick={() => pick(user.id, user.username)} role="option" type="button">
                    <span aria-hidden className="admin-check-box">{user.id === value ? <Check className="h-3.5 w-3.5" /> : null}</span>
                    <span className="admin-menu-text">
                      <b>{user.username}</b>
                      <small>{user.email}</small>
                    </span>
                  </button>
                ))}
                {results && !results.length ? <p className="admin-search-empty">{translate('admin.noPeopleMatch')}</p> : null}
                {!results && !value ? <p className="admin-search-empty">{translate('admin.typeToFindPerson')}</p> : null}
              </div>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

// ── Time ranges ───────────────────────────────────────────────────────────

const DAY = 86_400_000;
const WITHIN: Record<string, number> = { '24h': DAY, '7d': 7 * DAY, '30d': 30 * DAY, '90d': 90 * DAY };
const OLDER: Record<string, number> = { gt30d: 30 * DAY, gt90d: 90 * DAY };

/** A window in epoch ms for a filter value; `never` asks for rows never seen. */
export function timeWindow(value: string | null, now = Date.now()): { from?: number; to?: number; never?: boolean } {
  if (!value) return {};
  if (value === 'never') return { never: true };
  if (WITHIN[value]) return { from: now - WITHIN[value] };
  if (OLDER[value]) return { to: now - OLDER[value] };
  const match = /^(\d{4}-\d{2}-\d{2})?\.\.(\d{4}-\d{2}-\d{2})?$/.exec(value);
  if (match) {
    const start = match[1] ? localMidnight(match[1]) : undefined;
    const end = match[2] ? localMidnight(match[2]) + DAY : undefined;
    return { ...(start !== undefined ? { from: start } : {}), ...(end !== undefined ? { to: end } : {}) };
  }
  return {};
}

function localMidnight(day: string) {
  const [year, month, date] = day.split('-').map(Number);
  return new Date(year!, (month ?? 1) - 1, date ?? 1).getTime();
}

function shortDay(day: string) {
  return new Intl.DateTimeFormat(getLocale(), { month: 'short', day: 'numeric' }).format(localMidnight(day));
}

export function timeLabel(value: string | null) {
  if (!value) return null;
  switch (value) {
    case '24h': return translate('admin.within24h');
    case '7d': return translate('admin.within7d');
    case '30d': return translate('admin.within30d');
    case '90d': return translate('admin.within90d');
    case 'gt30d': return translate('admin.olderThan30d');
    case 'gt90d': return translate('admin.olderThan90d');
    case 'never': return translate('admin.never');
  }
  const match = /^(\d{4}-\d{2}-\d{2})?\.\.(\d{4}-\d{2}-\d{2})?$/.exec(value);
  if (!match) return value;
  if (match[1] && match[2]) return `${shortDay(match[1])} – ${shortDay(match[2])}`;
  if (match[1]) return translate('admin.sinceDay', { day: shortDay(match[1]) });
  if (match[2]) return translate('admin.untilDay', { day: shortDay(match[2]) });
  return value;
}

/** Adds a window to API parameters under `prefix` (`seen` → seenFrom/seenTo). */
export function windowParams(value: string | null, prefix: 'seen' | 'created' | '', now = Date.now()) {
  const window = timeWindow(value, now);
  const params: Record<string, string | number> = {};
  if (window.never && prefix === 'seen') params.seen = 'never';
  const name = (end: 'From' | 'To') => (prefix ? `${prefix}${end}` : end.toLowerCase());
  if (window.from !== undefined) params[name('From')] = Math.round(window.from);
  if (window.to !== undefined) params[name('To')] = Math.round(window.to);
  return params;
}

function today() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
}

/** A time window: presets ("Last 7 days", "More than 30 days ago", "Never") and a custom range. */
export function TimeFilter({ label, value, onChange, presets = ['24h', '7d', '30d', '90d'], older = false, never = false }: {
  label: string;
  value: string | null;
  onChange: (value: string | null) => void;
  presets?: string[];
  older?: boolean;
  never?: boolean;
}) {
  useI18n();
  const [custom, setCustom] = useState(false);
  const popover = usePopover(custom);
  const range = value && value.includes('..') ? value.split('..') : ['', ''];
  const [from, setFrom] = useState(range[0] ?? '');
  const [to, setTo] = useState(range[1] ?? '');
  useMenuKeyboard(popover.panel, popover.open && !custom, popover.close);
  useEffect(() => {
    if (!popover.open) setCustom(false);
    else {
      const [start, end] = value && value.includes('..') ? value.split('..') : ['', ''];
      setFrom(start ?? '');
      setTo(end ?? '');
    }
  }, [popover.open, value]);
  const pick = (next: string | null) => {
    onChange(next);
    popover.close(true);
  };
  const choices = [...presets, ...(older ? ['gt30d', 'gt90d'] : []), ...(never ? ['never'] : [])];
  const apply = (event: FormEvent) => {
    event.preventDefault();
    if (!from && !to) return;
    if (from && to && from > to) return;
    pick(`${from}..${to}`);
  };
  const invalid = Boolean(from && to && from > to);
  return (
    <>
      <PillButton buttonRef={popover.trigger} label={label} onClear={() => onChange(null)} onClick={() => popover.setOpen((open) => !open)} open={popover.open} valueLabel={timeLabel(value)} />
      {popover.open
        ? createPortal(
            custom ? (
              <div
                aria-label={label}
                className="admin-menu admin-filter-menu admin-range-form"
                onKeyDown={(event) => {
                  if (event.key === 'Escape') {
                    event.stopPropagation();
                    popover.close(true);
                  }
                }}
                ref={popover.panel}
                role="dialog"
                style={panelStyle(popover.place)}
              >
                <form onSubmit={apply}>
                  <label>
                    <span>{translate('admin.fromDay')}</span>
                    <input autoFocus className="relay-input" max={to || today()} onChange={(event) => setFrom(event.target.value)} type="date" value={from} />
                  </label>
                  <label>
                    <span>{translate('admin.toDay')}</span>
                    <input className="relay-input" max={today()} min={from || undefined} onChange={(event) => setTo(event.target.value)} type="date" value={to} />
                  </label>
                  {invalid ? <p className="admin-field-hint">{translate('admin.rangeOrder')}</p> : null}
                  <div className="admin-form-actions">
                    <button className="relay-button-secondary" onClick={() => setCustom(false)} type="button">{translate('admin.back')}</button>
                    <button className="relay-button-primary" disabled={(!from && !to) || invalid} type="submit">{translate('admin.apply')}</button>
                  </div>
                </form>
              </div>
            ) : (
              <div aria-label={label} className="admin-menu admin-filter-menu" ref={popover.panel} role="menu" style={panelStyle(popover.place)}>
                <button aria-checked={!value} onClick={() => pick(null)} role="menuitemradio" tabIndex={-1} type="button">
                  <span aria-hidden className="admin-check-box">{!value ? <Check className="h-3.5 w-3.5" /> : null}</span>
                  <span>{translate('admin.anyTime')}</span>
                </button>
                {choices.map((choice) => (
                  <button aria-checked={choice === value} key={choice} onClick={() => pick(choice)} role="menuitemradio" tabIndex={-1} type="button">
                    <span aria-hidden className="admin-check-box">{choice === value ? <Check className="h-3.5 w-3.5" /> : null}</span>
                    <span>{timeLabel(choice)}</span>
                  </button>
                ))}
                <span aria-hidden className="admin-menu-rule" />
                <button aria-checked={Boolean(value?.includes('..'))} onClick={() => setCustom(true)} role="menuitemradio" tabIndex={-1} type="button">
                  <span aria-hidden className="admin-check-box">{value?.includes('..') ? <Check className="h-3.5 w-3.5" /> : null}</span>
                  <span>{translate('admin.customRange')}</span>
                </button>
              </div>
            ),
            document.body,
          )
        : null}
    </>
  );
}
