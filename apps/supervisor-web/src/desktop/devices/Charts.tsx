import { useState, type ReactNode } from 'react';
import { compact, deviceColor, duration, exact, money, type StatsSummary } from './model';

export interface ChartSource { deviceId: string; name: string; summary: StatsSummary }
function Tip({ children }: { children: ReactNode }) { return <div className="pd-chart-tip" role="tooltip">{children}</div>; }
export function Sparkline({ days, color, label }: { days: StatsSummary['daily']; color: string; label: string }) {
  const [selected, setSelected] = useState<number | null>(null);
  const max = Math.max(1, ...days.map(d => d.turns));
  return <div className="pd-sparkline">
    <svg viewBox="0 0 168 26" role="group" aria-label={label} onMouseLeave={() => setSelected(null)}>
      {days.map((day, i) => <g key={day.date} tabIndex={0} aria-label={`${day.date}：${exact(day.turns)} 轮`} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
        <rect x={i * 12} y={0} width={11} height={26} fill="transparent" />
        <rect x={i * 12 + 2} y={24 - day.turns / max * 21} width={7} height={Math.max(2, day.turns / max * 21)} rx={2} fill={color} opacity={selected === i ? 1 : .65} />
      </g>)}
    </svg>
    {selected !== null && days[selected] ? <Tip>{days[selected]!.date} · {exact(days[selected]!.turns)} 轮</Tip> : null}
  </div>;
}

export interface HourlyChartSource { deviceId: string; name: string; values: (number | null)[] }
const hourLabel = (hour: number) => `${String(hour).padStart(2, '0')}:00`;
const hourRange = (hour: number) => `${hourLabel(hour)}–${hourLabel(hour + 1)}`;
const showTick = (index: number, length: number, hourly: boolean) => hourly ? index % 6 === 0 || index === 23 : index === 0 || index === length - 1 || length > 7 && index % 7 === 0 && index < length - 3;

function StackedChart({ sources, labels, ticks, label, hourly = false, runtime = false }: {
  sources: HourlyChartSource[]; labels: string[]; ticks: string[]; label: string; hourly?: boolean; runtime?: boolean;
}) {
  const [selected, setSelected] = useState<number | null>(null);
  const value = (source: HourlyChartSource, index: number) => source.values[index] ?? null;
  const maximum = Math.max(0, ...labels.map((_, i) => sources.reduce((sum, s) => sum + (value(s, i) ?? 0), 0)));
  const known = sources.some(source => source.values.some(value => value !== null));
  const scale = Math.max(1, maximum), left = 42, width = 620, baseline = 154, step = (width - left - 8) / Math.max(1, labels.length);
  const format = (v: number | null) => v == null ? '未提供数据' : runtime ? `≈ ${duration(v)}` : `${exact(v)} 轮`;
  return <div className="pd-chart-wrap">
    <svg viewBox="0 0 620 182" role="group" aria-label={label} onMouseLeave={() => setSelected(null)}>
      <line x1={left} x2={width - 8} y1={baseline} y2={baseline} className="pd-axis" />
      <text x={left - 8} y={32} textAnchor="end" className="pd-axis-label">{!known ? '—' : runtime ? `${compact(maximum / 60_000)}分` : compact(maximum)}</text>
      <text x={left - 8} y={baseline} textAnchor="end" className="pd-axis-label">0</text>
      {labels.map((period, i) => {
        let stack = 0;
        const barWidth = Math.min(44, step * .62);
        return <g key={period} tabIndex={0} aria-label={`${period}，${sources.map(s => `${s.name} ${format(value(s, i))}`).join('，')}`} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
          <rect x={left + i * step} width={step} y={16} height={baseline - 8} fill={selected === i ? 'var(--theme-hover)' : 'transparent'} rx={3} />
          {sources.map(source => { const height = (value(source, i) ?? 0) / scale * 124; stack += height; return <rect key={source.deviceId} data-chart-bar x={left + i * step + (step - barWidth) / 2} y={baseline - stack} width={barWidth} height={height} rx={Math.min(2, height / 2)} fill={deviceColor(source.deviceId)} />; })}
          {showTick(i, labels.length, hourly) ? <text x={left + (i + .5) * step} y={176} textAnchor="middle" className="pd-axis-label">{ticks[i]}</text> : null}
        </g>;
      })}
    </svg>
    {selected !== null ? <Tip><strong>{labels[selected]}</strong>{sources.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}<b>{format(value(s, selected))}</b></span>)}</Tip> : null}
    <div className="pd-legend">{sources.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}</span>)}</div>
    {runtime ? <div className="pd-chart-caption">活动采样估算 · 并行线程时长相加 · 未覆盖时段留空</div> : hourly ? <div className="pd-chart-caption">本地时间 · 按轮次开始时刻归组</div> : null}
  </div>;
}
export function TurnsChart({ sources, dates }: { sources: ChartSource[]; dates: string[] }) {
  return <StackedChart sources={sources.map(s => ({ deviceId: s.deviceId, name: s.name, values: dates.map(date => s.summary.daily.find(day => day.date === date)?.turns ?? null) }))} labels={dates} ticks={dates.map(date => date.slice(5).replace('-', '/'))} label="每日轮数，按设备堆叠" />;
}
export function HourlyActivityChart({ sources, runtime = false }: { sources: HourlyChartSource[]; runtime?: boolean }) {
  return <StackedChart sources={sources} labels={Array.from({ length: 24 }, (_, hour) => hourRange(hour))} ticks={Array.from({ length: 24 }, (_, hour) => hourLabel(hour))} label={runtime ? '今日每小时运行时长，按设备堆叠' : '今日每小时轮数，按设备堆叠'} hourly runtime={runtime} />;
}

export interface UsageBucket { date: string; tokens: number | null; costUsd: number | null }
export function UsageChart({ daily, mode, hourly = false }: { daily: UsageBucket[]; mode: 'tokens' | 'cost'; hourly?: boolean }) {
  const [selected, setSelected] = useState<number | null>(null);
  const values = daily.map(day => mode === 'tokens' ? day.tokens : day.costUsd);
  const maximum = Math.max(0, ...values.map(value => value ?? 0)), scale = Math.max(maximum, Number.EPSILON);
  const known = values.some(value => value !== null);
  const left = 42, baseline = 154, step = 570 / Math.max(1, daily.length), barWidth = Math.min(44, step * .62);
  const format = (value: number | null) => value == null ? (mode === 'tokens' ? '用量未知' : '费用未知') : mode === 'tokens' ? `${exact(value)} tokens` : `≈ $${value.toLocaleString('en-US', { maximumSignificantDigits: 12 })}`;
  const name = `${hourly ? '今日每小时' : '每日'}${mode === 'tokens' ? ' tokens' : '估算费用'}`;
  return <div className="pd-chart-wrap">
    <svg viewBox="0 0 620 182" role="group" aria-label={name} onMouseLeave={() => setSelected(null)}>
      <line x1={left} x2={612} y1={baseline} y2={baseline} className="pd-axis" />
      <text x={left - 8} y={32} textAnchor="end" className="pd-axis-label">{!known ? '—' : mode === 'tokens' ? compact(maximum) : money(maximum)}</text>
      <text x={left - 8} y={baseline} textAnchor="end" className="pd-axis-label">0</text>
      {daily.map((day, i) => {
        const value = values[i]!, height = (value ?? 0) / scale * 124;
        return <g key={day.date} tabIndex={0} aria-label={`${day.date}：${format(value)}`} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
          <rect x={left + i * step} y={16} width={step} height={146} fill={selected === i ? 'var(--theme-hover)' : 'transparent'} />
          {value !== null ? <rect data-chart-bar x={left + i * step + (step - barWidth) / 2} y={baseline - height} width={barWidth} height={height} rx={Math.min(2, height / 2)} fill="var(--theme-accent-solid)" /> : <path data-chart-unknown d={`M${left + i * step + (step - barWidth) / 2},150h${barWidth}`} className="pd-axis" strokeDasharray="2 2" />}
          {showTick(i, daily.length, hourly) ? <text x={left + (i + .5) * step} y={176} textAnchor="middle" className="pd-axis-label">{hourly ? hourLabel(i) : day.date.slice(5).replace('-', '/')}</text> : null}
        </g>;
      })}
    </svg>
    {selected !== null && daily[selected] ? <Tip><strong>{daily[selected]!.date}</strong>{format(values[selected]!)}</Tip> : null}
    <div className="pd-chart-caption">{hourly ? '每小时' : '每日'}独立用量 · {mode === 'tokens' ? '输入 + 输出，缓存读取已包含在输入中' : '估算费用，未知费用保留为空缺'}</div>
  </div>;
}
