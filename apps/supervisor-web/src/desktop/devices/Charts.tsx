import { useState, type ReactNode } from 'react';
import { formatNumber, useI18n } from '@pockymoe/thread-ui/i18n';
import { compact, deviceColor, duration, exact, money, turns, type StatsSummary } from './model';

export interface ChartSource { deviceId: string; name: string; summary: StatsSummary }
function Tip({ children, beside }: { children: ReactNode; beside?: { from: number; to: number } | undefined }) {
  // Beside the hovered column, on whichever side has room, so the tip never covers what it describes.
  const style = beside && (beside.from + beside.to < 1 ? { left: `calc(${beside.to * 100}% + 6px)`, right: 'auto' } : { right: `calc(${(1 - beside.from) * 100}% + 6px)`, left: 'auto' });
  return <div className="pd-chart-tip" role="tooltip" {...(style ? { style } : {})}>{children}</div>;
}
export function Sparkline({ days, color, label }: { days: StatsSummary['daily']; color: string; label: string }) {
  const { t } = useI18n();
  const [selected, setSelected] = useState<number | null>(null);
  const max = Math.max(1, ...days.map(d => d.turns));
  return <div className="pd-sparkline">
    <svg viewBox="0 0 168 26" role="group" aria-label={label} onMouseLeave={() => setSelected(null)}>
      {days.map((day, i) => <g key={day.date} tabIndex={0} aria-label={t('desktopDevices.chartValue', { period: day.date, values: turns(day.turns) })} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
        <rect x={i * 12} y={0} width={11} height={26} fill="transparent" />
        <rect x={i * 12 + 2} y={24 - day.turns / max * 21} width={7} height={Math.max(2, day.turns / max * 21)} rx={2} fill={color} opacity={selected === i ? 1 : .65} />
      </g>)}
    </svg>
    {selected !== null && days[selected] ? <Tip>{days[selected]!.date} · {turns(days[selected]!.turns)}</Tip> : null}
  </div>;
}

export interface HourlyChartSource { deviceId: string; name: string; values: (number | null)[] }
const hourLabel = (hour: number) => `${String(hour).padStart(2, '0')}:00`;
const hourRange = (hour: number) => `${hourLabel(hour)}–${hourLabel(hour + 1)}`;
const showTick = (index: number, length: number, hourly: boolean) => hourly ? index % 6 === 0 || index === 23 : index === 0 || index === length - 1 || length > 7 && index % 7 === 0 && index < length - 3;

function StackedChart({ sources, labels, ticks, label, hourly = false, runtime = false }: {
  sources: HourlyChartSource[]; labels: string[]; ticks: string[]; label: string; hourly?: boolean; runtime?: boolean;
}) {
  const { t } = useI18n();
  const [selected, setSelected] = useState<number | null>(null);
  const value = (source: HourlyChartSource, index: number) => source.values[index] ?? null;
  const maximum = Math.max(0, ...labels.map((_, i) => sources.reduce((sum, s) => sum + (value(s, i) ?? 0), 0)));
  const known = sources.some(source => source.values.some(value => value !== null));
  const scale = Math.max(1, maximum), left = 42, width = 620, baseline = 154, step = (width - left - 8) / Math.max(1, labels.length);
  const format = (v: number | null) => v == null ? t('desktopDevices.noData') : runtime ? `≈ ${duration(v)}` : turns(v);
  return <div className="pd-chart-wrap">
    <svg viewBox="0 0 620 182" role="group" aria-label={label} onMouseLeave={() => setSelected(null)}>
      <line x1={left} x2={width - 8} y1={baseline} y2={baseline} className="pd-axis" />
      <text x={left - 8} y={32} textAnchor="end" className="pd-axis-label">{!known ? '—' : runtime ? t('desktopDevices.minutesAxis', { value: compact(maximum / 60_000) }) : compact(maximum)}</text>
      <text x={left - 8} y={baseline} textAnchor="end" className="pd-axis-label">0</text>
      {labels.map((period, i) => {
        let stack = 0;
        const barWidth = Math.min(44, step * .62);
        return <g key={period} tabIndex={0} aria-label={t('desktopDevices.chartValue', { period, values: sources.map(s => `${s.name} ${format(value(s, i))}`).join(t('desktopDevices.listSeparator')) })} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
          <rect x={left + i * step} width={step} y={16} height={baseline - 8} fill={selected === i ? 'var(--theme-hover)' : 'transparent'} rx={3} />
          {sources.map(source => { const height = (value(source, i) ?? 0) / scale * 124; stack += height; return <rect key={source.deviceId} data-chart-bar x={left + i * step + (step - barWidth) / 2} y={baseline - stack} width={barWidth} height={height} rx={Math.min(2, height / 2)} fill={deviceColor(source.deviceId)} />; })}
          {showTick(i, labels.length, hourly) ? <text x={left + (i + .5) * step} y={176} textAnchor="middle" className="pd-axis-label">{ticks[i]}</text> : null}
        </g>;
      })}
    </svg>
    {selected !== null ? (() => {
      const rows = sources.filter(s => (value(s, selected) ?? 0) > 0).sort((a, b) => value(b, selected)! - value(a, selected)!);
      const unknown = sources.filter(s => value(s, selected) === null).length;
      return <Tip beside={{ from: (left + selected * step) / 620, to: (left + (selected + 1) * step) / 620 }}><strong>{labels[selected]}</strong>
        {rows.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}<b>{format(value(s, selected))}</b></span>)}
        {!rows.length && !unknown ? <span>{format(0)}</span> : null}
        {unknown ? <span className="pd-chart-tip-muted" title={sources.filter(s => value(s, selected) === null).map(s => s.name).join(t('desktopDevices.listSeparator'))}>{unknown} · {t('desktopDevices.noData')}</span> : null}
      </Tip>;
    })() : null}
    <div className="pd-legend">{sources.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}</span>)}</div>
    {runtime ? <div className="pd-chart-caption">{t('desktopDevices.runtimeCaption')}</div> : hourly ? <div className="pd-chart-caption">{t('desktopDevices.hourlyCaption')}</div> : null}
  </div>;
}
export function TurnsChart({ sources, dates }: { sources: ChartSource[]; dates: string[] }) {
  const { t } = useI18n();
  return <StackedChart sources={sources.map(s => ({ deviceId: s.deviceId, name: s.name, values: dates.map(date => s.summary.daily.find(day => day.date === date)?.turns ?? null) }))} labels={dates} ticks={dates.map(date => date.slice(5).replace('-', '/'))} label={t('desktopDevices.dailyTurnsChart')} />;
}
export function HourlyActivityChart({ sources, runtime = false }: { sources: HourlyChartSource[]; runtime?: boolean }) {
  const { t } = useI18n();
  return <StackedChart sources={sources} labels={Array.from({ length: 24 }, (_, hour) => hourRange(hour))} ticks={Array.from({ length: 24 }, (_, hour) => hourLabel(hour))} label={runtime ? t('desktopDevices.hourlyRuntimeChart') : t('desktopDevices.hourlyTurnsChart')} hourly runtime={runtime} />;
}

export interface UsageBucket { date: string; tokens: number | null; costUsd: number | null; pricePartial?: boolean; unpricedTokens?: number; partialUsageTurns?: number }
export function UsageChart({ daily, mode, hourly = false }: { daily: UsageBucket[]; mode: 'tokens' | 'cost'; hourly?: boolean }) {
  const { t } = useI18n();
  const [selected, setSelected] = useState<number | null>(null);
  const values = daily.map(day => mode === 'tokens' ? day.tokens : day.costUsd);
  const maximum = Math.max(0, ...values.map(value => value ?? 0)), scale = Math.max(maximum, Number.EPSILON);
  const known = values.some(value => value !== null);
  const left = 42, baseline = 154, step = 570 / Math.max(1, daily.length), barWidth = Math.min(44, step * .62);
  const format = (value: number | null, partial = false) => value == null ? (mode === 'tokens' ? t('desktopDevices.unknownUsage') : t('desktopDevices.unknownCost')) : mode === 'tokens' ? `${exact(value)} tokens` : `${partial ? '≥ ' : ''}$${formatNumber(value, { maximumSignificantDigits: 12 })}`;
  const name = t(hourly ? mode === 'tokens' ? 'desktopDevices.hourlyTokensChart' : 'desktopDevices.hourlyCostChart' : mode === 'tokens' ? 'desktopDevices.dailyTokensChart' : 'desktopDevices.dailyCostChart');
  return <div className="pd-chart-wrap">
    <svg viewBox="0 0 620 182" role="group" aria-label={name} onMouseLeave={() => setSelected(null)}>
      <line x1={left} x2={612} y1={baseline} y2={baseline} className="pd-axis" />
      <text x={left - 8} y={32} textAnchor="end" className="pd-axis-label">{!known ? '—' : mode === 'tokens' ? compact(maximum) : money(maximum)}</text>
      <text x={left - 8} y={baseline} textAnchor="end" className="pd-axis-label">0</text>
      {daily.map((day, i) => {
        const value = values[i]!, height = (value ?? 0) / scale * 124;
        return <g key={day.date} tabIndex={0} aria-label={t('desktopDevices.chartValue', { period: day.date, values: format(value, Boolean(day.pricePartial || day.unpricedTokens || day.partialUsageTurns)) })} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
          <rect x={left + i * step} y={16} width={step} height={146} fill={selected === i ? 'var(--theme-hover)' : 'transparent'} />
          {value !== null ? <rect data-chart-bar x={left + i * step + (step - barWidth) / 2} y={baseline - height} width={barWidth} height={height} rx={Math.min(2, height / 2)} fill="var(--theme-accent-solid)" /> : <path data-chart-unknown d={`M${left + i * step + (step - barWidth) / 2},150h${barWidth}`} className="pd-axis" strokeDasharray="2 2" />}
          {showTick(i, daily.length, hourly) ? <text x={left + (i + .5) * step} y={176} textAnchor="middle" className="pd-axis-label">{hourly ? hourLabel(i) : day.date.slice(5).replace('-', '/')}</text> : null}
        </g>;
      })}
    </svg>
    {selected !== null && daily[selected] ? <Tip beside={{ from: (left + selected * step) / 620, to: (left + (selected + 1) * step) / 620 }}><strong>{daily[selected]!.date}</strong>{format(values[selected]!, Boolean(daily[selected]!.pricePartial || daily[selected]!.unpricedTokens || daily[selected]!.partialUsageTurns))}</Tip> : null}
    <div className="pd-chart-caption">{t(hourly ? mode === 'tokens' ? 'desktopDevices.hourlyTokenCaption' : 'desktopDevices.hourlyCostCaption' : mode === 'tokens' ? 'desktopDevices.dailyTokenCaption' : 'desktopDevices.dailyCostCaption')}</div>
  </div>;
}
