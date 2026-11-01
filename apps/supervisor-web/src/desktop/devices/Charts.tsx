import { useState, type ReactNode } from 'react';
import { compact, deviceColor, exact, money, type StatsSummary } from './model';

export interface ChartSource { deviceId: string; name: string; summary: StatsSummary }
function Tip({ children }: { children: ReactNode }) { return <div className="rd-chart-tip" role="tooltip">{children}</div>; }
export function Sparkline({ days, color, label }: { days: StatsSummary['daily']; color: string; label: string }) {
  const [selected, setSelected] = useState<number | null>(null);
  const max = Math.max(1, ...days.map(d => d.turns));
  return <div className="rd-sparkline">
    <svg viewBox="0 0 168 26" role="group" aria-label={label} onMouseLeave={() => setSelected(null)}>
      {days.map((day, i) => <g key={day.date} tabIndex={0} aria-label={`${day.date}：${exact(day.turns)} 轮`} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
        <rect x={i * 12} y={0} width={11} height={26} fill="transparent" />
        <rect x={i * 12 + 2} y={24 - day.turns / max * 21} width={7} height={Math.max(2, day.turns / max * 21)} rx={2} fill={color} opacity={selected === i ? 1 : .65} />
      </g>)}
    </svg>
    {selected !== null && days[selected] ? <Tip>{days[selected]!.date} · {exact(days[selected]!.turns)} 轮</Tip> : null}
  </div>;
}
export function TurnsChart({ sources, dates }: { sources: ChartSource[]; dates: string[] }) {
  const [selected, setSelected] = useState<string | null>(null);
  const value = (source: ChartSource, date: string) => source.summary.daily.find(d => d.date === date)?.turns ?? 0;
  const max = Math.max(1, ...dates.map(date => sources.reduce((n, s) => n + value(s, date), 0)));
  const width = 620, baseline = 154, plotHeight = 124, left = 32, step = (width - left - 8) / Math.max(1, dates.length);
  return <div className="rd-chart-wrap">
    <svg viewBox={`0 0 ${width} 182`} role="group" aria-label="每日轮数，按设备堆叠" onMouseLeave={() => setSelected(null)}>
      <line x1={left} x2={width - 8} y1={baseline} y2={baseline} className="rd-axis" />
      <text x={left - 8} y={32} textAnchor="end" className="rd-axis-label">{compact(max)}</text>
      <text x={left - 8} y={baseline} textAnchor="end" className="rd-axis-label">0</text>
      {dates.map((date, i) => {
        let stack = 0;
        return <g key={date} tabIndex={0} aria-label={`${date}，${sources.map(s => `${s.name} ${exact(value(s, date))} 轮`).join('，')}`} onFocus={() => setSelected(date)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(date)}>
          <rect x={left + i * step} width={step} y={16} height={baseline - 8} fill={selected === date ? 'var(--rc-row-hover)' : 'transparent'} rx={3} />
          {sources.map(source => { const turns = value(source, date); const height = turns / max * plotHeight; stack += height; return <rect key={source.deviceId} x={left + i * step + step * .19} y={baseline - stack} width={step * .62} height={height} rx={Math.min(2, height / 2)} fill={deviceColor(source.deviceId)} />; })}
          {(i === 0 || i === dates.length - 1 || (dates.length > 7 && i % 7 === 0 && i < dates.length - 3)) ? <text x={left + (i + .5) * step} y={176} textAnchor="middle" className="rd-axis-label">{date.slice(5).replace('-', '/')}</text> : null}
        </g>;
      })}
    </svg>
    {selected ? <Tip><strong>{selected}</strong>{sources.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}<b>{exact(value(s, selected))} 轮</b></span>)}</Tip> : null}
    <div className="rd-legend">{sources.map(s => <span key={s.deviceId}><i style={{ background: deviceColor(s.deviceId) }} />{s.name}</span>)}</div>
  </div>;
}
export function UsageChart({ daily, mode }: { daily: { date: string; tokens: number; costUsd: number | null }[]; mode: 'tokens' | 'cost' }) {
  const [selected, setSelected] = useState<number | null>(null);
  const values = daily.map(d => mode === 'tokens' ? d.tokens : d.costUsd);
  const max = Math.max(1, ...values.map(v => v ?? 0));
  const x = (i: number) => 32 + (daily.length === 1 ? 280 : i * 570 / Math.max(1, daily.length - 1));
  const y = (v: number) => 154 - v / max * 124;
  let path = '', previousKnown = false;
  values.forEach((v, i) => { if (v == null) { previousKnown = false; return; } path += `${previousKnown ? 'L' : 'M'}${x(i)},${y(v)} `; previousKnown = true; });
  const format = (v: number | null) => mode === 'tokens' ? `${exact(v ?? 0)} tokens` : v == null ? '费用未知' : `≈ $${v.toLocaleString('en-US', { maximumFractionDigits: 20 })}`;
  return <div className="rd-chart-wrap">
    <svg viewBox="0 0 620 182" role="group" aria-label={mode === 'tokens' ? '每日 tokens' : '每日估算费用'} onMouseLeave={() => setSelected(null)}>
      <line x1={32} x2={608} y1={154} y2={154} className="rd-axis" />
      <text x={24} y={32} textAnchor="end" className="rd-axis-label">{mode === 'tokens' ? compact(max) : money(max)}</text>
      <text x={24} y={154} textAnchor="end" className="rd-axis-label">0</text>
      <path d={path} fill="none" stroke="var(--rc-run, #5287de)" strokeWidth={2.2} strokeLinejoin="round" />
      {daily.map((day, i) => <g key={day.date} tabIndex={0} aria-label={`${day.date}：${format(values[i]!)}`} onFocus={() => setSelected(i)} onBlur={() => setSelected(null)} onMouseEnter={() => setSelected(i)}>
        <rect x={x(i) - 570 / Math.max(2, daily.length) / 2} y={16} width={570 / Math.max(1, daily.length)} height={146} fill="transparent" />
        {values[i] != null && (selected === i || daily.length === 1) ? <circle cx={x(i)} cy={y(values[i]!)} r={4} fill="var(--rc-run, #5287de)" stroke="var(--theme-bg)" strokeWidth={2} /> : null}
        {(i === 0 || i === daily.length - 1 || (daily.length > 7 && i % 7 === 0 && i < daily.length - 3)) ? <text x={x(i)} y={176} textAnchor={i === 0 ? 'start' : i === daily.length - 1 ? 'end' : 'middle'} className="rd-axis-label">{day.date.slice(5).replace('-', '/')}</text> : null}
      </g>)}
    </svg>
    {selected !== null && daily[selected] ? <Tip><strong>{daily[selected]!.date}</strong>{format(values[selected]!)}</Tip> : null}
    <div className="rd-chart-caption">{mode === 'tokens' ? '输入 + 输出 · 缓存读取已包含在输入中' : '估算费用 · 未知费用保留为空缺'}</div>
  </div>;
}
