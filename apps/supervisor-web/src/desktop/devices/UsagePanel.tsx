import { useUsageColors } from './UsageColors';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { ChevronDown, ChevronLeft, ChevronRight } from 'lucide-react';
import { ErrorMark, MiddleName } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { compact, exact, type StatsRange, type TurnOrigin } from './model';
import { accountUsage, costLabel, metricValue, type UsageMetric, type UsageRow, type UsageSource } from './usage';
import type { DesktopIndex } from '../data/types';

export function Info({ message, label }: { message: string; label?: string | undefined }) {
  return <ErrorMark className="pd-info" kind="unavailable" message={message} label={label ?? message} announce={false} />;
}
/** Devices per page in the usage list. */
const DEVICES_PER_PAGE = 8;
export function UsagePanel({ sources, index, range, metric, onMetric, origin, filters, onShowAll }: {
  sources: UsageSource[]; index: DesktopIndex; range: StatsRange; metric: UsageMetric; onMetric(value: UsageMetric): void; origin: TurnOrigin | ''; filters?: ReactNode; onShowAll?(): void;
}) {
  const { t, formatNumber } = useI18n();
  const data = accountUsage(sources, index, range, metric, origin);
  const { deviceColor, usageColor } = useUsageColors(index.devices.map(device => device.id).concat(sources.map(source => source.deviceId)), data.models.map(row => row.id));
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const anchor = useRef<{ element: HTMLElement; top: number } | null>(null);
  useLayoutEffect(() => {
    if (anchor.current) {
      const { element, top } = anchor.current;
      // Restore the clicked row after scroll anchoring/clamping, including collapse.
      let parent = element.parentElement;
      while (parent && parent.scrollHeight <= parent.clientHeight) parent = parent.parentElement;
      const delta = element.getBoundingClientRect().top - top;
      if (delta) { if (parent) parent.scrollTop += delta; else window.scrollBy(0, delta); }
      anchor.current = null;
    }
  }, [expanded]);
  const [focused, setFocused] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const modelTotal = data.models.reduce((sum, row) => sum + metricValue(row, metric), 0);
  const maximum = Math.max(1, ...data.devices.map(row => metricValue(row, metric)));
  const value = (row: UsageRow) => row.unknown || metric === 'tokens' && row.tokenUnknown && !row.tokens ? '—' : metric === 'tokens' ? compact(row.tokens) : costLabel(row);
  const priceInfo = (row: UsageRow) => row.costUsd == null ? t('desktopDevices.priceUnknown') : row.unpricedTokens ? t('desktopDevices.priceCoverage', { tokens: exact(row.unpricedTokens) }) : t('desktopDevices.unpricedUnknown');
  const breakdown = (row: UsageRow) => row.tokenUnknown && !row.tokens ? t('desktopDevices.unknownUsage') : t('desktopDevices.usageBreakdown', { input: row.inputTokens == null ? '—' : exact(row.inputTokens), read: row.cachedTokens == null ? '—' : exact(row.cachedTokens), write: row.cacheWriteTokens == null ? '—' : exact(row.cacheWriteTokens), output: row.outputTokens == null ? '—' : exact(row.outputTokens), reasoning: row.reasoningTokens == null ? '—' : exact(row.reasoningTokens) });
  const originHint = t('desktopDevices.turnOrigins', Object.fromEntries(Object.entries(data.origins).map(([key, count]) => [key, exact(count)])));
  const focusedRow = data.models.find(row => row.id === focused);
  const exactCost = (row: UsageRow) => row.costUsd == null ? '—' : `${row.unpricedTokens || row.partial ? '≥ ' : ''}$${formatNumber(row.costUsd, { maximumSignificantDigits: 12 })}`;
  const fullValue = (row: UsageRow) => row.unknown || metric === 'tokens' && row.tokenUnknown && !row.tokens ? '—' : metric === 'tokens' ? `${exact(row.tokens)} ${t('desktopDevices.tokens')}` : exactCost(row);
  const modelName = (row: UsageRow) => <span className="pd-model-name" data-muted={row.id === '<synthetic>' || undefined}><i style={{ background: usageColor(row.id) }} /><MiddleName name={row.label || t('desktopDevices.unknownModel')} /><Info label={t('desktopDevices.fullModelName')} message={row.label} /></span>;
  const usageNumber = (content: ReactNode, info?: ReactNode) => <span className="pd-usage-number"><span>{content}</span><span>{info}</span></span>;
  const modelNumbers = (row: UsageRow) => <><td data-label={t('desktopDevices.turnsColumn')} title={originHint}>{compact(row.turns)}</td><td data-label={t('desktopDevices.tokens')}>{usageNumber(<span title={exact(row.tokens)}>{row.tokenUnknown && !row.tokens ? '—' : compact(row.tokens)}</span>, <Info message={breakdown(row)} />)}</td><td data-label={t('desktopDevices.cost')} title={exactCost(row)}>{usageNumber(costLabel(row), row.costUsd == null || row.unpricedTokens || row.partial ? <Info message={priceInfo(row)} /> : null)}</td></>;
  const emptyNote = modelTotal ? null : metric === 'cost' && data.total.costUsd == null ? t('desktopDevices.priceUnknown') : data.total.tokenUnknown ? t('desktopDevices.unknownUsage') : !data.models.length ? t('desktopDevices.noModels') : null;
  // A device with no usage in this range is not news, and one with no statistics is only counted.
  const used = data.devices.filter(row => !row.unknown && metricValue(row, metric) > 0);
  const missing = data.devices.filter(row => row.unknown).length;
  const pages = Math.max(1, Math.ceil(used.length / DEVICES_PER_PAGE)), shownPage = Math.min(page, pages - 1);
  const visible = used.slice(shownPage * DEVICES_PER_PAGE, (shownPage + 1) * DEVICES_PER_PAGE);
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
        {data.models.map(row => <tr key={row.id} data-active={focused === row.id || undefined} onMouseEnter={() => setFocused(row.id)} onMouseLeave={() => setFocused(null)}><th scope="row" aria-label={row.label}>{modelName(row)}</th>{modelNumbers(row)}<td data-label={t('desktopDevices.share')}>{modelTotal && (metric !== 'cost' || row.costUsd != null) ? `${(metricValue(row, metric) / modelTotal * 100).toFixed(1)}%` : '—'}</td></tr>)}
        {!data.models.length ? <tr><td colSpan={5} className="pd-table-empty">{t(data.total.unknown ? 'desktopDevices.usageNeedsUpdate' : data.total.turns ? 'desktopDevices.noModels' : 'desktopDevices.noUsage')}{data.partial && !data.total.unknown ? <Info message={t('desktopDevices.usagePartial')} /> : null}{data.total.unknown && range !== 'all' && onShowAll ? <button className="pd-link" onClick={onShowAll}>{t('desktopDevices.usageShowAll')}</button> : null}</td></tr> : null}
      </tbody><tfoot><tr><th scope="row">{t('desktopDevices.total')}</th><td data-label={t('desktopDevices.turnsColumn')} title={originHint}>{data.total.unknown ? '—' : compact(data.total.turns)}</td><td data-label={t('desktopDevices.tokens')} title={exact(data.total.tokens)}>{usageNumber(data.total.unknown || data.total.tokenUnknown && !data.total.tokens ? '—' : compact(data.total.tokens))}</td><td data-label={t('desktopDevices.cost')} title={exactCost(data.total)}>{usageNumber(costLabel(data.total), data.total.unpricedTokens || data.total.partial || data.total.costUsd == null ? <Info message={priceInfo(data.total)} /> : null)}</td><td /></tr></tfoot></table>
    </div>
    <section className="pd-device-usage" aria-label={t('desktopDevices.usageByDevice')}><header><h3>{t('desktopDevices.usageByDevice')}</h3>{data.partial ? <Info message={t('desktopDevices.usagePartial')} /> : null}</header>
      {!used.length && !missing ? <p className="pd-row-empty">{t('desktopDevices.usageNoneInRange')}</p> : null}
      {visible.map(device => <div className="pd-usage-device" key={device.id}>
        <button className="pd-usage-device-row" aria-expanded={expanded.has(device.id)} aria-label={`${device.label} · ${fullValue(device)}`} title={`${device.label} · ${fullValue(device)}`} onClick={event => { anchor.current = { element: event.currentTarget, top: event.currentTarget.getBoundingClientRect().top }; setExpanded(previous => { const next = new Set(previous); if (next.has(device.id)) next.delete(device.id); else next.add(device.id); return next; }); }}>
          {expanded.has(device.id) ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}<span className="pd-usage-label" title={device.label}>{device.label}</span><span className="pd-horizontal-track"><i style={{ width: `${metricValue(device, metric) / maximum * 100}%`, background: deviceColor(device.id) }} /></span><b>{value(device)}</b></button>
        {expanded.has(device.id) ? <div className="pd-device-details">
          <section aria-label={`${device.label} · ${t('desktopDevices.model')}`}><h4>{t('desktopDevices.model')}</h4>
            {device.modelHistory ? device.models.length ? <table className="pd-usage-table pd-device-models"><thead><tr><th scope="col">{t('desktopDevices.model')}</th><th scope="col">{t('desktopDevices.turnsColumn')}</th><th scope="col" aria-sort={metric === 'tokens' ? 'descending' : 'none'}>{t('desktopDevices.tokens')}</th><th scope="col" aria-sort={metric === 'cost' ? 'descending' : 'none'}>{t('desktopDevices.cost')}</th></tr></thead><tbody>{device.models.map(row => <tr key={row.id}><th scope="row" aria-label={row.label}>{modelName(row)}</th>{modelNumbers(row)}</tr>)}</tbody></table> : <p className="pd-row-empty">{t('desktopDevices.noModels')}</p> : <p className="pd-row-empty">{t('desktopDevices.noDeviceModels')}</p>}
          </section>
          <section className="pd-workspace-usage" aria-label={`${device.label} · ${t('desktopDevices.workspaceUsage')}`}><h4>{t('desktopDevices.workspace')}</h4>{device.workspaces.map(workspace => <Link key={workspace.id} to={`/devices/${encodeURIComponent(device.id)}/threads?workspaceId=${encodeURIComponent(workspace.id)}`}><span title={workspace.label}>{workspace.label}</span><span className="pd-horizontal-track"><i style={{ width: `${metricValue(workspace, metric) / maximum * 100}%`, background: deviceColor(device.id) }} /></span><b title={fullValue(workspace)}>{value(workspace)}</b></Link>)}{!device.workspaces.length ? <span className="pd-row-empty">{t('desktopDevices.noWorkspaces')}</span> : null}</section>
        </div> : null}
      </div>)}
      {pages > 1 || missing ? <footer className="pd-usage-footer">
        {missing ? <span className="pd-usage-missing">{t('desktopDevices.usageMissingDevices', { count: missing })}<Info message={t('desktopDevices.usageMissingHint')} /></span> : <span />}
        {pages > 1 ? <span className="pd-pager"><button aria-label={t('desktopDevices.usagePrevious')} title={t('desktopDevices.usagePrevious')} disabled={shownPage === 0} onClick={() => setPage(shownPage - 1)}><ChevronLeft size={13} aria-hidden /></button><span>{t('desktopDevices.usagePage', { page: shownPage + 1, pages })}</span><button aria-label={t('desktopDevices.usageNext')} title={t('desktopDevices.usageNext')} disabled={shownPage === pages - 1} onClick={() => setPage(shownPage + 1)}><ChevronRight size={13} aria-hidden /></button></span> : null}
      </footer> : null}
    </section>
  </section>;
}
