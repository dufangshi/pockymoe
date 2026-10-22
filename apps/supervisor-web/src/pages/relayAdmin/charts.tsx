// Small inline SVG charts for the admin console. Thin marks, recessive grid,
// series colors from the theme (--admin-chart-1…5), text in text tokens, a
// tooltip on hover and keyboard focus, and a table view for every chart.
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Table2, ChartColumn } from 'lucide-react';
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
import { formatCount, formatDateTime } from './format';

export type SeriesSlot = 1 | 2 | 3 | 4 | 5;

export interface ChartTable {
  columns: string[];
  rows: Array<Array<string | number>>;
}

/** Rounds a maximum up to a clean axis and returns the ticks. */
export function niceTicks(max: number, count = 4, integer = true) {
  if (!(max > 0)) return [0, 1];
  const raw = max / count;
  const power = 10 ** Math.floor(Math.log10(raw));
  const nice = [1, 2, 2.5, 5, 10].map((factor) => factor * power).find((candidate) => candidate >= raw) ?? raw;
  // Counts have whole-number ticks: no "0.5 people".
  const step = integer ? Math.max(1, Math.round(nice)) : nice;
  const top = Math.ceil(max / step) * step;
  const ticks: number[] = [];
  for (let value = 0; value <= top + step / 2; value += step) ticks.push(Number(value.toFixed(6)));
  return ticks;
}

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    setWidth(element.clientWidth);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(([entry]) => setWidth(Math.round(entry?.contentRect.width ?? 0)));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

/** A chart card: title, subtitle, optional note and a switch to its table. */
export function ChartCard({
  title,
  subtitle,
  aside,
  table,
  children,
  className,
  footer,
}: {
  title: string;
  subtitle?: ReactNode;
  aside?: ReactNode;
  table?: ChartTable | null;
  children: ReactNode;
  className?: string;
  footer?: ReactNode;
}) {
  useI18n();
  const [asTable, setAsTable] = useState(false);
  const headingId = useId();
  return (
    <section aria-labelledby={headingId} className={`admin-card admin-chart-card${className ? ` ${className}` : ''}`}>
      <header className="admin-card-header">
        <div className="admin-card-heading">
          <h3 id={headingId}>{title}</h3>
          {subtitle ? <p>{subtitle}</p> : null}
        </div>
        <div className="admin-card-aside">
          {aside}
          {table ? (
            <button
              aria-label={asTable ? translate('admin.showChart') : translate('admin.showTable')}
              aria-pressed={asTable}
              className="admin-icon-button"
              onClick={() => setAsTable((value) => !value)}
              title={asTable ? translate('admin.showChart') : translate('admin.showTable')}
              type="button"
            >
              {asTable ? <ChartColumn aria-hidden className="h-4 w-4" /> : <Table2 aria-hidden className="h-4 w-4" />}
            </button>
          ) : null}
        </div>
      </header>
      {asTable && table ? <DataTableView table={table} caption={title} /> : children}
      {footer ? <footer className="admin-card-footer">{footer}</footer> : null}
    </section>
  );
}

export function DataTableView({ table, caption }: { table: ChartTable; caption: string }) {
  return (
    <div className="admin-chart-table" role="region" aria-label={caption} tabIndex={0}>
      <table>
        <caption className="sr-only">{caption}</caption>
        <thead>
          <tr>
            {table.columns.map((column) => (
              <th key={column} scope="col">{column}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {table.rows.map((row, index) => (
            <tr key={index}>
              {row.map((cell, cellIndex) => (
                cellIndex === 0 ? <th key={cellIndex} scope="row">{cell}</th> : <td key={cellIndex}>{typeof cell === 'number' ? formatCount(cell) : cell}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** Where history begins after the range starts: say so, do not draw zeros. */
export function HistoryNote({ historyStart, rangeStart }: { historyStart: string | number | null | undefined; rangeStart: number }) {
  useI18n();
  const start = typeof historyStart === 'number' ? historyStart : historyStart ? Date.parse(historyStart) : null;
  if (start === null || !Number.isFinite(start) || start <= rangeStart) return null;
  return (
    <p className="admin-history-note">
      {translate('admin.historyStartsAt', { time: formatDateTime(start) })}
    </p>
  );
}

export function Legend({ items }: { items: Array<{ label: string; slot: SeriesSlot; value?: string }> }) {
  return (
    <ul className="admin-legend">
      {items.map((item) => (
        <li key={item.label}>
          <span aria-hidden className={`admin-swatch admin-series-${item.slot}`} />
          <span>{item.label}</span>
          {item.value ? <b>{item.value}</b> : null}
        </li>
      ))}
    </ul>
  );
}

interface TooltipState {
  index: number;
  x: number;
}

function useChartPointer(count: number) {
  const [active, setActive] = useState<TooltipState | null>(null);
  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLElement>, positionOf: (index: number) => number) => {
      if (!count) return;
      let next: number | null = null;
      const current = active?.index ?? -1;
      if (event.key === 'ArrowRight') next = Math.min(count - 1, current + 1);
      else if (event.key === 'ArrowLeft') next = Math.max(0, current < 0 ? count - 1 : current - 1);
      else if (event.key === 'Home') next = 0;
      else if (event.key === 'End') next = count - 1;
      else if (event.key === 'Escape') {
        setActive(null);
        return;
      }
      if (next === null) return;
      event.preventDefault();
      setActive({ index: next, x: positionOf(next) });
    },
    [active, count],
  );
  return { active, setActive, onKeyDown };
}

function Tooltip({ x, width, children }: { x: number; width: number; children: ReactNode }) {
  const left = Math.min(Math.max(x, 70), Math.max(70, width - 70));
  return (
    <div aria-hidden className="admin-tooltip" style={{ left }}>
      {children}
    </div>
  );
}

const PAD = { top: 12, right: 40, bottom: 24, left: 36 };

export interface LinePoint {
  at: number;
  value: number | null;
}

/**
 * One line with a light area under it, over time. Gaps (null) break the
 * line: they are unknown, not zero. The last point carries its value.
 */
export function LineChart({
  points,
  label,
  slot = 1,
  height = 200,
  formatValue = formatCount,
  formatAt,
  start,
  end,
  minMax = 4,
}: {
  points: LinePoint[];
  label: string;
  slot?: SeriesSlot;
  height?: number;
  formatValue?: (value: number) => string;
  formatAt: (at: number) => string;
  start?: number;
  end?: number;
  minMax?: number;
}) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const known = points.filter((point) => point.value !== null) as Array<{ at: number; value: number }>;
  const max = Math.max(minMax, ...known.map((point) => point.value));
  const ticks = niceTicks(max);
  const top = ticks[ticks.length - 1] ?? max;
  const from = start ?? points[0]?.at ?? 0;
  const to = end ?? points[points.length - 1]?.at ?? 1;
  const plotWidth = Math.max(10, width - PAD.left - PAD.right);
  const plotHeight = height - PAD.top - PAD.bottom;
  const x = (at: number) => PAD.left + ((at - from) / Math.max(1, to - from)) * plotWidth;
  const y = (value: number) => PAD.top + plotHeight - (value / top) * plotHeight;
  const runs: Array<Array<{ at: number; value: number }>> = [];
  let run: Array<{ at: number; value: number }> = [];
  for (const point of points) {
    if (point.value === null) {
      if (run.length) runs.push(run);
      run = [];
    } else run.push(point as { at: number; value: number });
  }
  if (run.length) runs.push(run);
  const line = runs.map((segment) => `M${segment.map((point) => `${x(point.at).toFixed(1)},${y(point.value).toFixed(1)}`).join('L')}`).join('');
  const area = runs
    .filter((segment) => segment.length > 1)
    .map((segment) => `M${x(segment[0]!.at).toFixed(1)},${y(0)}L${segment.map((point) => `${x(point.at).toFixed(1)},${y(point.value).toFixed(1)}`).join('L')}L${x(segment[segment.length - 1]!.at).toFixed(1)},${y(0)}Z`)
    .join('');
  const last = known[known.length - 1];
  const xTicks = Array.from({ length: 5 }, (_, index) => from + ((to - from) * index) / 4);
  const pointer = useChartPointer(points.length);
  const nearest = (px: number) => {
    let best = 0;
    let distance = Infinity;
    points.forEach((point, index) => {
      const d = Math.abs(x(point.at) - px);
      if (d < distance) {
        distance = d;
        best = index;
      }
    });
    return best;
  };
  const activePoint = pointer.active ? points[pointer.active.index] : null;
  const summary = last ? `${label}: ${formatValue(last.value)} (${formatAt(last.at)})` : label;
  return (
    <div
      className="admin-chart"
      onKeyDown={(event) => pointer.onKeyDown(event, (index) => x(points[index]!.at))}
      onPointerLeave={() => pointer.setActive(null)}
      onPointerMove={(event) => {
        const rect = event.currentTarget.getBoundingClientRect();
        const index = nearest(event.clientX - rect.left);
        pointer.setActive({ index, x: x(points[index]!.at) });
      }}
      onBlur={() => pointer.setActive(null)}
      ref={ref}
      tabIndex={0}
    >
      {width > 0 ? (
        <svg aria-label={summary} height={height} role="img" width={width}>
          {ticks.map((tick) => (
            <g key={tick}>
              <line className="admin-gridline" x1={PAD.left} x2={width - PAD.right} y1={y(tick)} y2={y(tick)} />
              <text className="admin-axis" textAnchor="end" x={PAD.left - 6} y={y(tick) + 4}>{formatValue(tick)}</text>
            </g>
          ))}
          {xTicks.map((tick, index) => (
            <text className="admin-axis" key={index} textAnchor={index === 0 ? 'start' : index === 4 ? 'end' : 'middle'} x={x(tick)} y={height - 6}>{formatAt(tick)}</text>
          ))}
          <path className={`admin-area admin-series-${slot}`} d={area} />
          <path className={`admin-line admin-series-${slot}`} d={line} />
          {last ? (
            <>
              <circle className={`admin-dot admin-series-${slot}`} cx={x(last.at)} cy={y(last.value)} r={4} />
              <text className="admin-value-label" x={x(last.at) + 8} y={y(last.value) + 4}>{formatValue(last.value)}</text>
            </>
          ) : null}
          {activePoint ? (
            <line className="admin-crosshair" x1={x(activePoint.at)} x2={x(activePoint.at)} y1={PAD.top} y2={PAD.top + plotHeight} />
          ) : null}
          {activePoint?.value !== null && activePoint ? (
            <circle className={`admin-dot admin-series-${slot}`} cx={x(activePoint.at)} cy={y(activePoint.value)} r={4} />
          ) : null}
        </svg>
      ) : (
        <div style={{ height }} />
      )}
      {activePoint && pointer.active ? (
        <Tooltip width={width} x={pointer.active.x}>
          <b>{activePoint.value === null ? translate('admin.noData') : formatValue(activePoint.value)}</b>
          <span>{formatAt(activePoint.at)}</span>
        </Tooltip>
      ) : null}
    </div>
  );
}

export interface BarDatum {
  key: string;
  label: string;
  /** Segment values by series key; a plain bar has one segment. */
  values: Record<string, number>;
}

/**
 * Columns, plain or stacked by series. Segments are separated by a 2px gap;
 * the tallest column carries its total.
 */
export function ColumnChart({
  data,
  series,
  label,
  height = 200,
  formatValue = formatCount,
  labelEvery,
}: {
  data: BarDatum[];
  series: Array<{ key: string; label: string; slot: SeriesSlot }>;
  label: string;
  height?: number;
  formatValue?: (value: number) => string;
  labelEvery?: number;
}) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const totals = data.map((datum) => series.reduce((sum, item) => sum + (datum.values[item.key] ?? 0), 0));
  const max = Math.max(1, ...totals);
  const ticks = niceTicks(max);
  const top = ticks[ticks.length - 1] ?? max;
  const right = 12;
  const plotWidth = Math.max(10, width - PAD.left - right);
  const plotHeight = height - PAD.top - PAD.bottom;
  const band = plotWidth / Math.max(1, data.length);
  const barWidth = Math.max(2, Math.min(24, band * 0.62));
  const y = (value: number) => PAD.top + plotHeight - (value / top) * plotHeight;
  const center = (index: number) => PAD.left + band * index + band / 2;
  const every = labelEvery ?? Math.max(1, Math.ceil(data.length / Math.max(2, Math.floor(plotWidth / 56))));
  const peak = totals.indexOf(Math.max(...totals));
  const pointer = useChartPointer(data.length);
  const active = pointer.active ? data[pointer.active.index] : null;
  const summary = `${label}: ${data.map((datum, index) => `${datum.label} ${formatValue(totals[index] ?? 0)}`).join(', ')}`;
  return (
    <div
      className="admin-chart"
      onBlur={() => pointer.setActive(null)}
      onKeyDown={(event) => pointer.onKeyDown(event, center)}
      onPointerLeave={() => pointer.setActive(null)}
      onPointerMove={(event) => {
        const rect = event.currentTarget.getBoundingClientRect();
        const index = Math.min(data.length - 1, Math.max(0, Math.floor((event.clientX - rect.left - PAD.left) / band)));
        if (data.length) pointer.setActive({ index, x: center(index) });
      }}
      ref={ref}
      tabIndex={0}
    >
      {width > 0 ? (
        <svg aria-label={summary} height={height} role="img" width={width}>
          {ticks.map((tick) => (
            <g key={tick}>
              <line className="admin-gridline" x1={PAD.left} x2={width - right} y1={y(tick)} y2={y(tick)} />
              <text className="admin-axis" textAnchor="end" x={PAD.left - 6} y={y(tick) + 4}>{formatValue(tick)}</text>
            </g>
          ))}
          {data.map((datum, index) => {
            let base = 0;
            const segments = series
              .map((item) => ({ item, value: datum.values[item.key] ?? 0 }))
              .filter((segment) => segment.value > 0);
            return (
              <g className={pointer.active?.index === index ? 'is-active' : undefined} key={datum.key}>
                {segments.map(({ item, value }, segmentIndex) => {
                  const y0 = y(base);
                  const y1 = y(base + value);
                  base += value;
                  // A 2px surface gap above every segment but the top one.
                  const gap = segmentIndex < segments.length - 1 ? 2 : 0;
                  const h = Math.max(0, y0 - y1 - gap);
                  const isTop = segmentIndex === segments.length - 1;
                  return (
                    <path
                      className={`admin-bar admin-series-${item.slot}`}
                      d={roundedTop(center(index) - barWidth / 2, y1 + (isTop ? 0 : gap), barWidth, h, isTop ? Math.min(4, barWidth / 2, h) : 0)}
                      key={item.key}
                    />
                  );
                })}
                {index % every === 0 || index === data.length - 1 ? (
                  <text className="admin-axis" textAnchor="middle" x={center(index)} y={height - 6}>{datum.label}</text>
                ) : null}
              </g>
            );
          })}
          {totals.length && (totals[peak] ?? 0) > 0 ? (
            <text className="admin-value-label" textAnchor="middle" x={center(peak)} y={y(totals[peak] ?? 0) - 5}>{formatValue(totals[peak] ?? 0)}</text>
          ) : null}
        </svg>
      ) : (
        <div style={{ height }} />
      )}
      {active && pointer.active ? (
        <Tooltip width={width} x={pointer.active.x}>
          <span>{active.label}</span>
          {series.length > 1 ? (
            series.map((item) => (
              <span className="admin-tooltip-row" key={item.key}>
                <i className={`admin-key admin-series-${item.slot}`} />
                <b>{formatValue(active.values[item.key] ?? 0)}</b>
                {item.label}
              </span>
            ))
          ) : (
            <b>{formatValue(active.values[series[0]?.key ?? ''] ?? 0)}</b>
          )}
        </Tooltip>
      ) : null}
    </div>
  );
}

function roundedTop(x: number, y: number, width: number, height: number, radius: number) {
  if (height <= 0) return '';
  const r = Math.min(radius, height);
  return `M${x},${y + height}V${y + r}Q${x},${y} ${x + r},${y}H${x + width - r}Q${x + width},${y} ${x + width},${y + r}V${y + height}Z`;
}

/** Labelled horizontal bars that share one scale. */
export function BarList({
  items,
  label,
  max,
  formatValue = formatCount,
}: {
  items: Array<{ key: string; label: string; value: number; slot?: SeriesSlot }>;
  label: string;
  max?: number;
  formatValue?: (value: number) => string;
}) {
  const top = Math.max(1, max ?? Math.max(0, ...items.map((item) => item.value)));
  return (
    <ul aria-label={label} className="admin-barlist">
      {items.map((item) => (
        <li key={item.key}>
          <span className="admin-barlist-label">{item.label}</span>
          <span aria-hidden className="admin-barlist-track">
            <span className={`admin-barlist-fill admin-series-${item.slot ?? 1}`} style={{ width: `${Math.max(item.value > 0 ? 1.5 : 0, (item.value / top) * 100)}%` }} />
          </span>
          <b className="admin-barlist-value">{formatValue(item.value)}</b>
        </li>
      ))}
    </ul>
  );
}

/** Parts of a whole as one horizontal strip with a legend under it. */
export function ProportionBar({
  items,
  label,
  formatValue = formatCount,
}: {
  items: Array<{ key: string; label: string; value: number; slot: SeriesSlot }>;
  label: string;
  formatValue?: (value: number) => string;
}) {
  const total = items.reduce((sum, item) => sum + item.value, 0);
  const visible = items.filter((item) => item.value > 0);
  return (
    <div className="admin-proportion">
      <div aria-label={`${label}: ${items.map((item) => `${item.label} ${formatValue(item.value)}`).join(', ')}`} className="admin-proportion-bar" role="img">
        {visible.map((item) => (
          <span className={`admin-series-${item.slot}`} key={item.key} style={{ flexGrow: item.value }} title={`${item.label}: ${formatValue(item.value)}`} />
        ))}
        {!total ? <span className="is-empty" /> : null}
      </div>
      <Legend items={items.map((item) => ({ label: item.label, slot: item.slot, value: formatValue(item.value) }))} />
    </div>
  );
}

/** A tiny trend line for table cells and figures, no axes. */
export function Sparkline({ values, label, slot = 1, width = 96, height = 24 }: { values: Array<number | null>; label: string; slot?: SeriesSlot; width?: number; height?: number }) {
  const known = values.filter((value): value is number => value !== null);
  if (!known.length) return <span className="admin-muted">—</span>;
  const max = Math.max(1, ...known);
  const step = values.length > 1 ? (width - 6) / (values.length - 1) : 0;
  const points = values
    .map((value, index) => (value === null ? null : [3 + index * step, height - 3 - (value / max) * (height - 6)] as const))
    .filter((point): point is readonly [number, number] => point !== null);
  const line = `M${points.map(([px, py]) => `${px.toFixed(1)},${py.toFixed(1)}`).join('L')}`;
  const last = points[points.length - 1];
  return (
    <svg aria-label={label} className="admin-sparkline" height={height} role="img" width={width}>
      <path className={`admin-area admin-series-${slot}`} d={`${line}L${last?.[0] ?? 0},${height - 2}L${points[0]?.[0] ?? 0},${height - 2}Z`} />
      <path className={`admin-line is-thin admin-series-${slot}`} d={line} />
      {last ? <circle className={`admin-dot admin-series-${slot}`} cx={last[0]} cy={last[1]} r={2.5} /> : null}
    </svg>
  );
}

export interface TimelineSpan {
  from: number;
  to: number;
  state: 'online' | 'unstable' | 'offline' | 'unknown';
}

/** Connection state across a time window as one strip; a legend names states. */
export function TimelineStrip({ spans, start, end, label, formatAt }: { spans: TimelineSpan[]; start: number; end: number; label: string; formatAt: (at: number) => string }) {
  useI18n();
  const length = Math.max(1, end - start);
  return (
    <div className="admin-timeline">
      <div aria-label={label} className="admin-timeline-strip" role="img">
        {spans.map((span, index) => (
          <span
            className={`is-${span.state}`}
            key={index}
            style={{ left: `${((span.from - start) / length) * 100}%`, width: `${Math.max(0.4, ((span.to - span.from) / length) * 100)}%` }}
            title={`${formatAt(span.from)} – ${formatAt(span.to)}`}
          />
        ))}
      </div>
      <div className="admin-timeline-axis" aria-hidden>
        <span>{formatAt(start)}</span>
        <span>{formatAt(start + length / 2)}</span>
        <span>{translate('admin.now')}</span>
      </div>
    </div>
  );
}

/** Reacts to a media query; false during the first server-less render. */
export function useMediaQuery(queryText: string) {
  const get = () => typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia(queryText).matches;
  const [matches, setMatches] = useState(get);
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(queryText);
    const update = () => setMatches(media.matches);
    update();
    media.addEventListener?.('change', update);
    return () => media.removeEventListener?.('change', update);
  }, [queryText]);
  return matches;
}
