import { useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { compact, deviceColor, exact, type StatsRange, type TurnOrigin } from './model';
import { accountUsage, costLabel, metricValue, usageColor, type UsageMetric, type UsageRow, type UsageSource } from './usage';
import type { DesktopIndex } from '../data/types';

export function Info({ message, label }: { message: string; label?: string | undefined }) {
  return <ErrorMark className="pd-info" kind="unavailable" message={message} label={label ?? message} announce={false} />;
}
export function UsagePanel({ sources, index, range, metric, onMetric, origin, filters }: {
  sources: UsageSource[]; index: DesktopIndex; range: StatsRange; metric: UsageMetric; onMetric(value: UsageMetric): void; origin: TurnOrigin | ''; filters?: ReactNode;
}) {
  const { t, formatNumber } = useI18n();
  const data = accountUsage(sources, index, range, metric, origin);
  const [focused, setFocused] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const modelTotal = data.models.reduce((sum, row) => sum + metricValue(row, metric), 0);
  const maximum = Math.max(1, ...data.devices.map(row => metricValue(row, metric)));
  const value = (row: UsageRow) => row.unknown || metric === 'tokens' && row.tokenUnknown && !row.tokens ? '—' : metric === 'tokens' ? compact(row.tokens) : costLabel(row);
  const priceInfo = (row: UsageRow) => row.costUsd == null ? t('desktopDevices.priceUnknown') : row.unpricedTokens ? t('desktopDevices.priceCoverage', { tokens: exact(row.unpricedTokens) }) : t('desktopDevices.unpricedUnknown');
  const breakdown = (row: UsageRow) => row.tokenUnknown && !row.tokens ? t('desktopDevices.unknownUsage') : t('desktopDevices.usageBreakdown', { input: row.inputTokens == null ? '—' : exact(row.inputTokens), read: row.cachedTokens == null ? '—' : exact(row.cachedTokens), write: row.cacheWriteTokens == null ? '—' : exact(row.cacheWriteTokens), output: row.outputTokens == null ? '—' : exact(row.outputTokens), reasoning: row.reasoningTokens == null ? '—' : exact(row.reasoningTokens) });
  const originHint = t('desktopDevices.turnOrigins', Object.fromEntries(Object.entries(data.origins).map(([key, count]) => [key, exact(count)])));
  const focusedRow = data.models.find(row => row.id === focused);
  const exactCost = (row: UsageRow) => row.costUsd == null ? '—' : `${row.unpricedTokens || row.partial ? '≥ ' : ''}$${formatNumber(row.costUsd, { maximumSignificantDigits: 12 })}`;
  const fullValue = (row: UsageRow) => row.unknown || metric === 'tokens' && row.tokenUnknown && !row.tokens ? '—' : metric === 'tokens' ? `${exact(row.tokens)} ${t('desktopDevices.tokens')}` : exactCost(row);
  const emptyNote = modelTotal ? null : metric === 'cost' && data.total.costUsd == null ? t('desktopDevices.priceUnknown') : data.total.tokenUnknown ? t('desktopDevices.unknownUsage') : !data.models.length ? t('desktopDevices.noModels') : null;
  let offset = 0;
  return <section className="pd-account-usage" aria-label={t('desktopDevices.accountUsage')}>
    <header className="pd-usage-header"><h2>{t('desktopDevices.accountUsage')}</h2><Info message={`${t('desktopDevices.usageDefinition')} ${originHint}${data.partial ? ` ${t('desktopDevices.usagePartial')}` : ''}`} /><div className="pd-usage-filters">{filters}<div className="pd-segment" role="group" aria-label={t('desktopDevices.usageMetric')}><button aria-pressed={metric === 'tokens'} onClick={() => onMetric('tokens')}>{t('desktopDevices.byTokens')}</button><button aria-pressed={metric === 'cost'} onClick={() => onMetric('cost')}>{t('desktopDevices.byCost')}</button></div></div></header>
    <div className="pd-account-models">
      <div className="pd-donut-wrap"><svg className="pd-donut" viewBox="0 0 180 180" role="group" aria-label={t('desktopDevices.models')} onMouseLeave={() => setFocused(null)}>
        <circle cx="90" cy="90" r="64" fill="none" stroke="var(--theme-hover)" strokeWidth="22" />
        {data.models.filter(row => metricValue(row, metric) > 0).map(row => {
          const share = metricValue(row, metric) / modelTotal, start = offset; offset += share;
          const label = `${row.label} · ${metric === 'tokens' ? exact(row.tokens) : exactCost(row)} · ${(share * 100).toFixed(1)}%`;
          return <circle key={row.id} cx="90" cy="90" r="64" fill="none" stroke={usageColor(row.id)} strokeWidth={focused === row.id ? 26 : 22} pathLength="1" strokeDasharray={`${share} ${1 - share}`} strokeDashoffset={-start} transform="rotate(-90 90 90)" tabIndex={0} aria-label={label} onFocus={() => setFocused(row.id)} onBlur={() => setFocused(null)} onMouseEnter={() => setFocused(row.id)}><title>{label}</title></circle>;
        })}
        <text x="90" y="86" textAnchor="middle" className="pd-donut-value" style={{ fontSize: metric === 'cost' ? 17 : 21 }}>{value(data.total)}</text><text x="90" y="105" textAnchor="middle" className="pd-donut-caption">{t(metric === 'tokens' ? 'desktopDevices.tokens' : 'desktopDevices.cost')}</text>
      </svg>{focusedRow ? <div role="tooltip" className="pd-donut-tip">{focusedRow.label} · {metric === 'tokens' ? exact(focusedRow.tokens) : exactCost(focusedRow)}</div> : null}
      {emptyNote ? <span className="pd-donut-empty">{emptyNote}</span> : null}</div>
      <table className="pd-usage-table"><thead><tr><th scope="col">{t('desktopDevices.model')}</th><th scope="col">{t('desktopDevices.turnsColumn')}</th><th scope="col" aria-sort={metric === 'tokens' ? 'descending' : 'none'}>{t('desktopDevices.tokens')}</th><th scope="col" aria-sort={metric === 'cost' ? 'descending' : 'none'}>{t('desktopDevices.cost')}</th><th scope="col">{t('desktopDevices.share')}</th></tr></thead><tbody>
        {data.models.map(row => <tr key={row.id} data-active={focused === row.id || undefined} onMouseEnter={() => setFocused(row.id)} onMouseLeave={() => setFocused(null)}><th scope="row"><span className="pd-model-name" title={row.label}><i style={{ background: usageColor(row.id) }} />{row.label || t('desktopDevices.unknownModel')}</span></th><td title={originHint}>{compact(row.turns)}</td><td><span title={exact(row.tokens)}>{row.tokenUnknown && !row.tokens ? '—' : compact(row.tokens)}</span><Info message={breakdown(row)} /></td><td title={exactCost(row)}>{costLabel(row)}{row.costUsd == null || row.unpricedTokens || row.partial ? <Info message={priceInfo(row)} /> : null}</td><td>{modelTotal && (metric !== 'cost' || row.costUsd != null) ? `${(metricValue(row, metric) / modelTotal * 100).toFixed(1)}%` : '—'}</td></tr>)}
        {!data.models.length ? <tr><td colSpan={5} className="pd-table-empty">{t(data.total.turns ? 'desktopDevices.noModels' : 'desktopDevices.noUsage')}{data.partial ? <Info message={t('desktopDevices.usagePartial')} /> : null}</td></tr> : null}
      </tbody><tfoot><tr><th scope="row">{t('desktopDevices.total')}</th><td title={originHint}>{data.total.unknown ? '—' : compact(data.total.turns)}</td><td title={exact(data.total.tokens)}>{data.total.unknown || data.total.tokenUnknown && !data.total.tokens ? '—' : compact(data.total.tokens)}</td><td title={exactCost(data.total)}>{costLabel(data.total)}{data.total.unpricedTokens || data.total.partial || data.total.costUsd == null ? <Info message={priceInfo(data.total)} /> : null}</td><td /></tr></tfoot></table>
    </div>
    <section className="pd-device-usage" aria-label={t('desktopDevices.usageByDevice')}><header><h3>{t('desktopDevices.usageByDevice')}</h3>{data.partial ? <Info message={t('desktopDevices.usagePartial')} /> : null}</header>
      {data.devices.map(device => <div className="pd-usage-device" key={device.id}>
        <button className="pd-usage-device-row" aria-expanded={expanded.has(device.id)} aria-label={`${device.label} · ${fullValue(device)}`} title={`${device.label} · ${fullValue(device)}`} onClick={() => setExpanded(previous => { const next = new Set(previous); if (next.has(device.id)) next.delete(device.id); else next.add(device.id); return next; })}>
          {expanded.has(device.id) ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}<span className="pd-usage-label" title={device.label}>{device.label}</span><span className="pd-horizontal-track"><i style={{ width: `${metricValue(device, metric) / maximum * 100}%`, background: deviceColor(device.id) }} /></span><b>{value(device)}</b></button>
        {expanded.has(device.id) ? <div className="pd-workspace-usage" aria-label={`${device.label} · ${t('desktopDevices.workspaceUsage')}`}>{device.workspaces.map(workspace => <Link key={workspace.id} to={`/devices/${encodeURIComponent(device.id)}/threads?workspaceId=${encodeURIComponent(workspace.id)}`}><span title={workspace.label}>{workspace.label}</span><span className="pd-horizontal-track"><i style={{ width: `${metricValue(workspace, metric) / maximum * 100}%`, background: deviceColor(device.id) }} /></span><b title={fullValue(workspace)}>{value(workspace)}</b></Link>)}{!device.workspaces.length ? <span className="pd-row-empty">{t('desktopDevices.noWorkspaces')}</span> : null}</div> : null}
      </div>)}
    </section>
  </section>;
}
