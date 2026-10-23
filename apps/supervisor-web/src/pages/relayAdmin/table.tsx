// The one table of the admin console. Every list uses it: sortable headers
// (click cycles ascending, descending, none, with a small arrow), resizable
// columns (drag the header's edge, double-click or Enter on it to fit the
// content, arrows to nudge), a column chooser, and a list of rows on phones.
// Widths, hidden columns and the sort are kept per table in localStorage; the
// sort of a page's main table is also in the URL (`sort`, `dir`).
import { useMenuKeyboard } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ArrowDown, ArrowDownUp, ArrowUp, Check, ChevronRight, Columns3, RotateCcw } from 'lucide-react';
import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { EmptyState, useIsPhone } from './ui';

export type SortDir = 'asc' | 'desc';
export interface SortState {
  key: string;
  dir: SortDir;
}

export interface TableColumn<T> {
  key: string;
  /** The header text, also the name in the column chooser. */
  label: string;
  /** Header content when it differs from the label (the label stays its accessible name). */
  header?: ReactNode;
  render: (row: T) => ReactNode;
  /** Sortable: a comparator sorts the rows on the page; otherwise the page asks the relay. */
  sortable?: boolean;
  compare?: (a: T, b: T) => number;
  /** The direction of the first click: newest or largest first for times and counts. */
  firstDir?: SortDir;
  /** Default width in px. */
  width?: number;
  minWidth?: number;
  numeric?: boolean;
  /** False keeps the column out of the chooser (the row's name, the menu). */
  hideable?: boolean;
  defaultHidden?: boolean;
  /** No resize handle (the menu column). */
  fixed?: boolean;
  className?: string;
}

interface StoredTable {
  widths?: Record<string, number>;
  hidden?: string[];
  shown?: string[];
  sort?: SortState | null;
}

const STORAGE_PREFIX = 'pockymoe-admin-table:';

function readStored(id: string): StoredTable {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_PREFIX + id) ?? '{}') as StoredTable;
    return value && typeof value === 'object' ? value : {};
  } catch {
    return {};
  }
}

function writeStored(id: string, value: StoredTable) {
  try {
    localStorage.setItem(STORAGE_PREFIX + id, JSON.stringify(value));
  } catch {
    /* Private windows may refuse storage; the table still works. */
  }
}

/** The next sort after a header click: first direction, the other, none. */
export function nextSort(current: SortState | null, key: string, firstDir: SortDir = 'asc'): SortState | null {
  if (!current || current.key !== key) return { key, dir: firstDir };
  if (current.dir === firstDir) return { key, dir: firstDir === 'asc' ? 'desc' : 'asc' };
  return null;
}

export function parseSort(sort: string | null | undefined, dir: string | null | undefined): SortState | null {
  if (!sort) return null;
  return { key: sort, dir: dir === 'desc' ? 'desc' : 'asc' };
}

/**
 * Per-table settings. `urlSort` (the page's main table) reads and writes the
 * sort in the URL; without it the sort lives only in storage.
 */
export function useTableSettings(
  id: string,
  options: {
    urlSort?: { sort: SortState | null; set: (sort: SortState | null) => void };
    defaultSort?: SortState | null;
  } = {},
) {
  const [stored, setStored] = useState<StoredTable>(() => readStored(id));
  const update = useCallback((patch: Partial<StoredTable>) => {
    setStored((current) => {
      const next = { ...current, ...patch };
      writeStored(id, next);
      return next;
    });
  }, [id]);
  const url = options.urlSort;
  const sort = url ? url.sort ?? (stored.sort !== undefined ? stored.sort : options.defaultSort ?? null) : stored.sort !== undefined ? stored.sort : options.defaultSort ?? null;
  const setSort = useCallback((next: SortState | null) => {
    update({ sort: next });
    url?.set(next);
  }, [update, url]);
  return {
    sort,
    setSort,
    widths: stored.widths ?? {},
    hidden: stored.hidden ?? [],
    shown: stored.shown ?? [],
    setWidth: (key: string, width: number | null) => {
      const widths = { ...(stored.widths ?? {}) };
      if (width === null) delete widths[key];
      else widths[key] = Math.round(width);
      update({ widths });
    },
    setVisible: (key: string, visible: boolean, defaultHidden: boolean) => {
      const hidden = new Set(stored.hidden ?? []);
      const shown = new Set(stored.shown ?? []);
      if (visible) {
        hidden.delete(key);
        if (defaultHidden) shown.add(key);
      } else {
        shown.delete(key);
        if (!defaultHidden) hidden.add(key);
      }
      update({ hidden: [...hidden], shown: [...shown] });
    },
    reset: () => {
      update({ widths: {}, hidden: [], shown: [] });
    },
  };
}

export type TableSettings = ReturnType<typeof useTableSettings>;

function isVisible<T>(column: TableColumn<T>, settings: TableSettings) {
  if (column.hideable === false) return true;
  if (settings.hidden.includes(column.key)) return false;
  if (column.defaultHidden) return settings.shown.includes(column.key);
  return true;
}

/** Sorts rows on the page with the column's comparator (when it has one). */
export function sortRows<T>(rows: T[], columns: Array<TableColumn<T>>, sort: SortState | null) {
  if (!sort) return rows;
  const column = columns.find((item) => item.key === sort.key);
  if (!column?.compare) return rows;
  const sorted = rows.slice().sort(column.compare);
  return sort.dir === 'desc' ? sorted.reverse() : sorted;
}

/** The text width of a cell's content as laid out, ignoring ellipsis. */
function contentWidth(cell: HTMLElement) {
  const left = cell.getBoundingClientRect().left;
  let right = left;
  const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT);
  const range = document.createRange();
  for (let node: Node | null = walker.currentNode; node; node = walker.nextNode()) {
    if (node.nodeType === Node.TEXT_NODE) {
      if (!node.textContent?.trim()) continue;
      range.selectNodeContents(node);
      for (const rect of Array.from(range.getClientRects())) right = Math.max(right, rect.right);
    } else if (node instanceof SVGSVGElement || node instanceof HTMLImageElement || (node instanceof HTMLElement && node.matches('input, button:empty, .admin-device-state-mark, .admin-avatar'))) {
      right = Math.max(right, node.getBoundingClientRect().right);
    }
  }
  return right - left;
}

/**
 * A table whose rows may open details. The first column usually holds a real
 * button (the row's name) so keyboards and screen readers open it; a click
 * anywhere on the row does the same. On phones the rows are a list
 * (`phoneRow`), with a sort menu instead of headers.
 */
export function AdminTable<T>({
  id,
  columns,
  rows,
  rowKey,
  onOpen,
  selected,
  phoneRow,
  label,
  empty,
  busy,
  settings,
  grow,
  toolbar,
  total,
}: {
  /** Names the stored settings; one per table. */
  id: string;
  columns: Array<TableColumn<T>>;
  rows: T[];
  rowKey: (row: T) => string;
  onOpen?: (row: T) => void;
  selected?: string | null;
  phoneRow?: (row: T) => ReactNode;
  label: string;
  empty: ReactNode;
  busy?: boolean;
  settings: TableSettings;
  /** The column that takes the free width while it has no width of its own. */
  grow?: string;
  /** Shown before the column chooser (above the table). */
  toolbar?: ReactNode;
  /** How many rows match (all pages), shown at the left of the tools. */
  total?: number | undefined;
}) {
  useI18n();
  const phone = useIsPhone();
  const visible = columns.filter((column) => isVisible(column, settings));
  const growKey = grow ?? columns[0]?.key;
  const hideable = columns.filter((column) => column.hideable !== false);
  const tools = (
    <div className="admin-table-tools">
      {total !== undefined ? <span className="admin-table-count" role="status">{translate('admin.resultsCount', { count: total })}</span> : null}
      {toolbar}
      {phone ? (
        <SortMenu columns={columns} label={label} settings={settings} />
      ) : hideable.length ? (
        <ColumnChooser columns={hideable} label={label} settings={settings} />
      ) : null}
    </div>
  );
  if (phone && phoneRow) {
    return (
      <>
        {tools}
        {!rows.length ? <EmptyState>{empty}</EmptyState> : (
          <ul aria-busy={busy || undefined} aria-label={label} className={`admin-list${busy ? ' is-busy' : ''}`}>
            {rows.map((row) => (
              <li key={rowKey(row)}>
                {onOpen ? (
                  <button aria-current={selected === rowKey(row) || undefined} className="admin-list-row" onClick={() => onOpen(row)} type="button">
                    {phoneRow(row)}
                    <ChevronRight aria-hidden className="admin-list-chevron h-4 w-4" />
                  </button>
                ) : (
                  <div className="admin-list-row">{phoneRow(row)}</div>
                )}
              </li>
            ))}
          </ul>
        )}
      </>
    );
  }
  return (
    <>
      {tools}
      <TableGrid
        busy={busy}
        columns={visible}
        empty={empty}
        growKey={growKey}
        label={label}
        onOpen={onOpen}
        rowKey={rowKey}
        rows={rows}
        selected={selected}
        settings={settings}
        tableId={id}
      />
    </>
  );
}

function TableGrid<T>({ tableId, columns, rows, rowKey, onOpen, selected, label, empty, busy, settings, growKey }: {
  tableId: string;
  columns: Array<TableColumn<T>>;
  rows: T[];
  rowKey: (row: T) => string;
  onOpen?: ((row: T) => void) | undefined;
  selected?: string | null | undefined;
  label: string;
  empty: ReactNode;
  busy?: boolean | undefined;
  settings: TableSettings;
  growKey: string | undefined;
}) {
  const tableRef = useRef<HTMLTableElement>(null);
  // Live widths while a handle is dragged; stored when the drag ends.
  const [dragging, setDragging] = useState<{ key: string; width: number } | null>(null);
  const widthOf = (column: TableColumn<T>) => {
    if (dragging?.key === column.key) return dragging.width;
    const stored = settings.widths[column.key];
    if (stored) return stored;
    if (column.key === growKey && !column.fixed) return null;
    return column.width ?? 140;
  };
  const growFree = columns.some((column) => column.key === growKey && widthOf(column) === null);
  const minWidth = columns.reduce((sum, column) => sum + (widthOf(column) ?? Math.max(column.minWidth ?? 120, column.width ?? 200)), 0);
  const fit = (column: TableColumn<T>) => {
    const table = tableRef.current;
    if (!table) return;
    let width = 0;
    table.querySelectorAll<HTMLElement>(`[data-col="${CSS.escape(column.key)}"] > .admin-cell`).forEach((cell) => {
      width = Math.max(width, contentWidth(cell));
    });
    // Cell padding (2 × 14px) and the sort arrow's room.
    settings.setWidth(column.key, Math.max(column.minWidth ?? 64, Math.ceil(width + 34)));
  };
  return (
    <div aria-label={label} className={`admin-table-wrap${busy ? ' is-busy' : ''}`} role="region" tabIndex={0}>
      <table aria-busy={busy || undefined} className="admin-table is-fixed" data-table={tableId} ref={tableRef} style={{ minWidth }}>
        <colgroup>
          {columns.map((column) => {
            const width = widthOf(column);
            return <col key={column.key} style={width === null ? undefined : { width }} />;
          })}
          {!growFree ? <col className="admin-col-filler" /> : null}
        </colgroup>
        <thead>
          <tr>
            {columns.map((column, index) => (
              <HeaderCell
                column={column}
                key={column.key}
                last={index === columns.length - 1 && growFree}
                onFit={() => fit(column)}
                onResize={(width, done) => {
                  if (done) {
                    setDragging(null);
                    settings.setWidth(column.key, width);
                  } else setDragging({ key: column.key, width });
                }}
                settings={settings}
                width={() => {
                  const th = tableRef.current?.querySelector<HTMLElement>(`th[data-col="${CSS.escape(column.key)}"]`);
                  return th?.getBoundingClientRect().width ?? widthOf(column) ?? 160;
                }}
              />
            ))}
            {!growFree ? <th aria-hidden className="admin-col-filler" /> : null}
          </tr>
        </thead>
        <tbody>
          {rows.length ? rows.map((row) => {
            const key = rowKey(row);
            return (
              <tr
                aria-selected={onOpen ? selected === key : undefined}
                className={onOpen ? 'is-openable' : undefined}
                key={key}
                onClick={(event) => {
                  if (!onOpen) return;
                  // Buttons and links inside the row act on their own.
                  if ((event.target as HTMLElement).closest('button, a, input')) return;
                  onOpen(row);
                }}
              >
                {columns.map((column) => (
                  <td className={[column.className, column.numeric ? 'is-numeric' : ''].filter(Boolean).join(' ') || undefined} data-col={column.key} key={column.key}>
                    <div className="admin-cell">{column.render(row)}</div>
                  </td>
                ))}
                {!growFree ? <td aria-hidden className="admin-col-filler" /> : null}
              </tr>
            );
          }) : (
            <tr className="is-empty">
              <td colSpan={columns.length + (growFree ? 0 : 1)}>
                <EmptyState>{empty}</EmptyState>
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

function HeaderCell<T>({ column, settings, onResize, onFit, width, last }: {
  column: TableColumn<T>;
  settings: TableSettings;
  onResize: (width: number, done: boolean) => void;
  onFit: () => void;
  width: () => number;
  last: boolean;
}) {
  useI18n();
  const sort = settings.sort;
  const active = sort?.key === column.key ? sort.dir : null;
  const sortable = column.sortable || Boolean(column.compare);
  const min = column.minWidth ?? 64;
  const drag = useRef<{ x: number; width: number } | null>(null);
  const onPointerDown = (event: ReactPointerEvent<HTMLSpanElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    drag.current = { x: event.clientX, width: width() };
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLSpanElement>) => {
    if (!drag.current) return;
    onResize(Math.max(min, drag.current.width + event.clientX - drag.current.x), false);
  };
  const onPointerUp = (event: ReactPointerEvent<HTMLSpanElement>) => {
    if (!drag.current) return;
    const next = Math.max(min, drag.current.width + event.clientX - drag.current.x);
    drag.current = null;
    onResize(next, true);
  };
  const onKeyDown = (event: ReactKeyboardEvent<HTMLSpanElement>) => {
    const step = event.shiftKey ? 48 : 16;
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault();
      onResize(Math.max(min, width() + (event.key === 'ArrowRight' ? step : -step)), true);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      onFit();
    }
  };
  const ariaSort = sortable ? (active === 'asc' ? 'ascending' : active === 'desc' ? 'descending' : 'none') : undefined;
  return (
    <th aria-sort={ariaSort} className={[column.className, column.numeric ? 'is-numeric' : ''].filter(Boolean).join(' ') || undefined} data-col={column.key} scope="col">
      {sortable ? (
        <button
          className={`admin-th-sort${active ? ' is-active' : ''}`}
          onClick={() => settings.setSort(nextSort(sort, column.key, column.firstDir ?? 'asc'))}
          title={translate('admin.sortBy', { column: column.label })}
          type="button"
        >
          <span className="admin-th-label">{column.header ?? column.label}</span>
          <span aria-hidden className="admin-sort-mark">
            {active === 'asc' ? <ArrowUp className="h-3.5 w-3.5" /> : active === 'desc' ? <ArrowDown className="h-3.5 w-3.5" /> : <ArrowDownUp className="h-3.5 w-3.5" />}
          </span>
        </button>
      ) : (
        <span className="admin-th-label">{column.header ?? column.label}</span>
      )}
      {!column.fixed && !last ? (
        <span
          aria-label={translate('admin.resizeColumn', { column: column.label })}
          aria-orientation="vertical"
          aria-valuemin={min}
          aria-valuenow={Math.round(settings.widths[column.key] ?? column.width ?? 0) || undefined}
          className="admin-col-resize"
          onDoubleClick={onFit}
          onKeyDown={onKeyDown}
          onPointerCancel={() => {
            drag.current = null;
          }}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          role="separator"
          tabIndex={0}
          title={translate('admin.resizeColumnHint')}
        />
      ) : null}
    </th>
  );
}

/** A popover anchored under a trigger, rendered at the page root. `layout`
    changes when the panel's content changes, to place it again. */
export function usePopover(layout?: unknown) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{ top: number; left: number; maxHeight: number } | null>(null);
  const close = useCallback((restore: boolean) => {
    setOpen(false);
    if (restore) trigger.current?.focus();
  }, []);
  useLayoutEffect(() => {
    if (!open) {
      setPlace(null);
      return;
    }
    const position = () => {
      const anchor = trigger.current?.getBoundingClientRect();
      const box = panel.current?.getBoundingClientRect();
      if (!anchor) return;
      const viewport = window.visualViewport;
      const height = viewport?.height ?? window.innerHeight;
      const widthAvailable = viewport?.width ?? window.innerWidth;
      const width = box?.width ?? 240;
      const below = height - anchor.bottom - 12;
      const above = anchor.top - 12;
      const up = below < 220 && above > below;
      const maxHeight = Math.max(160, up ? above : below);
      setPlace({
        top: up ? Math.max(8, anchor.top - 4 - Math.min(box?.height ?? 0, maxHeight)) : anchor.bottom + 4,
        left: Math.max(8, Math.min(anchor.left, widthAvailable - width - 8)),
        maxHeight,
      });
    };
    position();
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!panel.current?.contains(target) && !trigger.current?.contains(target)) setOpen(false);
    };
    // Anchored to a toolbar: it follows its trigger while the page scrolls
    // and closes once the trigger leaves the view (docs/ui-principles.md §3).
    const scroll = (event: Event) => {
      if (panel.current?.contains(event.target as Node)) return;
      const anchor = trigger.current?.getBoundingClientRect();
      const height = window.visualViewport?.height ?? window.innerHeight;
      if (!anchor || anchor.bottom < 0 || anchor.top > height) setOpen(false);
      else position();
    };
    const key = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        close(true);
      }
    };
    window.addEventListener('pointerdown', outside, true);
    window.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', position);
    window.visualViewport?.addEventListener('resize', position);
    panel.current?.addEventListener('keydown', key);
    const node = panel.current;
    return () => {
      window.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', position);
      window.visualViewport?.removeEventListener('resize', position);
      node?.removeEventListener('keydown', key);
    };
  }, [open, close, layout]);
  return { open, setOpen, trigger, panel, place, close };
}

/** Shows and hides columns; "Reset columns" brings back widths and defaults. */
function ColumnChooser<T>({ columns, settings, label }: { columns: Array<TableColumn<T>>; settings: TableSettings; label: string }) {
  useI18n();
  const popover = usePopover();
  useMenuKeyboard(popover.panel, popover.open, popover.close);
  const shownCount = columns.filter((column) => isVisible(column, settings)).length;
  return (
    <>
      <button
        aria-expanded={popover.open}
        aria-haspopup="menu"
        className="admin-tool-button"
        onClick={() => popover.setOpen((value) => !value)}
        ref={popover.trigger}
        title={translate('admin.columnsOf', { table: label })}
        type="button"
      >
        <Columns3 aria-hidden className="h-4 w-4" />
        <span>{translate('admin.columns')}</span>
      </button>
      {popover.open
        ? createPortal(
            <div aria-label={translate('admin.columnsOf', { table: label })} className="admin-menu admin-column-menu" ref={popover.panel} role="menu" style={popover.place ? { top: popover.place.top, left: popover.place.left, maxHeight: popover.place.maxHeight } : { top: -9999, left: -9999 }}>
              {columns.map((column) => {
                const shown = isVisible(column, settings);
                // The last shown column stays: a table needs one.
                const locked = shown && shownCount <= 1;
                return (
                  <button
                    aria-checked={shown}
                    disabled={locked}
                    key={column.key}
                    onClick={() => settings.setVisible(column.key, !shown, Boolean(column.defaultHidden))}
                    role="menuitemcheckbox"
                    tabIndex={-1}
                    type="button"
                  >
                    <span aria-hidden className="admin-check-box">{shown ? <Check className="h-3.5 w-3.5" /> : null}</span>
                    <span>{column.label}</span>
                  </button>
                );
              })}
              <span aria-hidden className="admin-menu-rule" />
              <button onClick={() => settings.reset()} role="menuitem" tabIndex={-1} type="button">
                <RotateCcw aria-hidden className="h-4 w-4" />
                <span>{translate('admin.resetColumns')}</span>
              </button>
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/** Phones: the sortable columns as one menu ("Sort: Last seen ↓"). */
function SortMenu<T>({ columns, settings, label }: { columns: Array<TableColumn<T>>; settings: TableSettings; label: string }) {
  useI18n();
  const popover = usePopover();
  useMenuKeyboard(popover.panel, popover.open, popover.close);
  const sortable = columns.filter((column) => column.sortable || column.compare);
  const current = sortable.find((column) => column.key === settings.sort?.key);
  if (!sortable.length) return null;
  return (
    <>
      <button
        aria-expanded={popover.open}
        aria-haspopup="menu"
        className={`admin-tool-button${current ? ' is-active' : ''}`}
        onClick={() => popover.setOpen((value) => !value)}
        ref={popover.trigger}
        type="button"
      >
        <ArrowDownUp aria-hidden className="h-4 w-4" />
        <span>{current ? translate('admin.sortedBy', { column: current.label }) : translate('admin.sort')}</span>
        {current ? (settings.sort?.dir === 'desc' ? <ArrowDown aria-label={translate('admin.descending')} className="h-3.5 w-3.5" /> : <ArrowUp aria-label={translate('admin.ascending')} className="h-3.5 w-3.5" />) : null}
      </button>
      {popover.open
        ? createPortal(
            <div aria-label={translate('admin.sortOf', { table: label })} className="admin-menu" ref={popover.panel} role="menu" style={popover.place ? { top: popover.place.top, left: popover.place.left, maxHeight: popover.place.maxHeight } : { top: -9999, left: -9999 }}>
              <button aria-checked={!settings.sort} onClick={() => { settings.setSort(null); popover.close(true); }} role="menuitemradio" tabIndex={-1} type="button">
                <span aria-hidden className="admin-check-box">{!settings.sort ? <Check className="h-3.5 w-3.5" /> : null}</span>
                <span>{translate('admin.defaultOrder')}</span>
              </button>
              {sortable.flatMap((column) => (['asc', 'desc'] as const).map((dir) => {
                const checked = settings.sort?.key === column.key && settings.sort.dir === dir;
                return (
                  <button aria-checked={checked} key={`${column.key}-${dir}`} onClick={() => { settings.setSort({ key: column.key, dir }); popover.close(true); }} role="menuitemradio" tabIndex={-1} type="button">
                    <span aria-hidden className="admin-check-box">{checked ? <Check className="h-3.5 w-3.5" /> : null}</span>
                    <span>{column.label}</span>
                    {dir === 'asc' ? <ArrowUp aria-label={translate('admin.ascending')} className="ml-auto h-3.5 w-3.5" /> : <ArrowDown aria-label={translate('admin.descending')} className="ml-auto h-3.5 w-3.5" />}
                  </button>
                );
              }))}
            </div>,
            document.body,
          )
        : null}
    </>
  );
}

/** What the table helpers read from a page's props. */
interface UrlProps {
  params: URLSearchParams;
  filter: string | null;
  setParams: (params: Record<string, string | null>) => void;
}

/** The page's filter values: the URL's, or the legacy filter's. */
export function useFilterValues<K extends string>(props: UrlProps, keys: readonly K[], legacy: (filter: string | null) => Partial<Record<K, string>>) {
  const { params, filter } = props;
  return useMemo(() => {
    const values = Object.fromEntries(keys.map((key) => [key, params.get(key)])) as Record<K, string | null>;
    return { ...values, ...legacy(filter) } as Record<K, string | null>;
  }, [params, filter, keys, legacy]);
}

/** The sort of a page's main table, kept in the URL (`sort`, `dir`). */
export function useUrlSort(props: UrlProps) {
  const { params, setParams } = props;
  return useMemo(() => ({
    sort: parseSort(params.get('sort'), params.get('dir')),
    set: (next: SortState | null) => setParams({ sort: next?.key ?? null, dir: next?.dir ?? null }),
  }), [params, setParams]);
}

