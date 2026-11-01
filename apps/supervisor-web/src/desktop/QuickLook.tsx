import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ExternalLink, X } from 'lucide-react';
import { ErrorMark, StatusMark, navigationStatusLabel, TokenUsageCost, useModalFocus } from '@pockymoe/thread-ui';
import { useDesktopSelector, useDesktopStore } from './data';
import { useDesktopSurface } from './context';
import { resolveRequest } from './integration';
import { workbenchThreadStatus } from '../pages/useWorkbenchNavigation';

export function QuickLook({ threadKey, onClose, onOpen }: { threadKey: string; onClose(): void; onOpen(key: string, newWindow?: boolean): void }) {
  const store = useDesktopStore();
  const surface = useDesktopSurface();
  const thread = useDesktopSelector(index => index.threads.find(thread => thread.key === threadKey));
  const activity = useDesktopSelector(index => thread ? index.activity[thread.deviceId]?.threads.find(item => item.threadId === thread.id) : undefined);
  const dialog = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 24, top: 72 });
  const [error, setError] = useState(''); const [busy, setBusy] = useState(false);
  useModalFocus(dialog, true, onClose, '[data-quicklook-close]');
  const deviceId = thread?.deviceId, threadId = thread?.id;
  useEffect(() => {
    if (!deviceId || !threadId) return;
    let alive = true;
    void store.loadDetail(deviceId, threadId).catch(error => { if (alive) setError(error instanceof Error ? error.message : String(error)); });
    return () => { alive = false; };
  }, [store, deviceId, threadId]);
  useEffect(() => {
    let frame = 0;
    const follow = () => {
      const anchor = document.querySelector<HTMLElement>(`[data-map-id="thread:${CSS.escape(threadKey)}"]`);
      const rect = anchor?.getBoundingClientRect();
      if (!rect || rect.bottom <= 0 || rect.top >= innerHeight || rect.right <= 0 || rect.left >= innerWidth) { onClose(); return; }
      const width = dialog.current?.offsetWidth ?? 420, height = dialog.current?.offsetHeight ?? 480;
      const left = rect.right + width + 12 < innerWidth ? rect.right + 12 : rect.left - width - 12 >= 12 ? rect.left - width - 12 : Math.max(12, Math.min(rect.right - width - 12, innerWidth - width - 12));
      const top = Math.max(12, Math.min(rect.top, innerHeight - height - 12));
      setPosition(old => old.left === left && old.top === top ? old : { left, top });
      frame = requestAnimationFrame(follow);
    };
    frame = requestAnimationFrame(follow); return () => cancelAnimationFrame(frame);
  }, [threadKey, onClose]);
  if (!thread) return null;
  const state = thread.requests.length ? 'waiting' : workbenchThreadStatus(thread, thread.lastViewedAt ?? thread.firstIndexedCompletedAt);
  const decide = async (decision: 'approve' | 'deny') => {
    const request = thread.requests[0]; if (!request) return;
    setBusy(true); setError('');
    try { await resolveRequest(thread.deviceId, thread.id, request.id, decision, { store }); }
    catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };
  const bars = activity?.hourlyMs ?? Array.from({ length: 24 }, () => 0);
  return createPortal(<div className="desktop-quicklook-layer" onPointerDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div ref={dialog} role="dialog" aria-modal="true" aria-label={`${thread.title} · 详情`} tabIndex={-1} className="thread-ui-shell thread-graph-dialog desktop-quicklook" data-theme-effective={document.documentElement.dataset.themeEffective} style={position}>
      <header><StatusMark status={state} /><h2>{thread.title}</h2><button data-quicklook-close className="thread-icon-button" aria-label="关闭详情" onClick={onClose}><X data-icon="close" size={16} /></button></header>
      <div className="desktop-quicklook-body">
        {thread.requests[0] ? <section className="desktop-quicklook-request"><h3>{thread.requests[0].kind === 'approval' ? '等待审批' : '等待输入'}</h3><p>{thread.requests[0].summary}</p>{thread.requests[0].kind === 'approval' ? <div className="desktop-decision-buttons"><button className="relay-button-primary" disabled={busy} onClick={() => { void decide('approve'); }}>{busy ? '处理中…' : '批准'}</button><button className="relay-button-secondary" disabled={busy} onClick={() => { void decide('deny'); }}>拒绝</button></div> : <button className="relay-button-secondary" onClick={() => onOpen(thread.key)}>打开线程回答</button>}</section> : null}
        <section><h3>最近的提问</h3><p>{activity?.lastPrompt || '暂无线程活动摘要'}</p></section>
        <dl><div><dt>模型</dt><dd>{thread.model || '—'}</dd></div><div><dt>轮数</dt><dd>{activity?.turns ?? '—'}</dd></div><div><dt>总运行时间</dt><dd>{activity ? `${Math.round(activity.totalMs / 60000)} 分钟` : '—'}</dd></div><div><dt>当前状态</dt><dd>{navigationStatusLabel(state)}</dd></div></dl>
        <section><h3>近 24 小时</h3><svg className="desktop-quicklook-bars" role="img" aria-label="每小时运行时间" viewBox="0 0 336 48">{bars.map((ms, i) => { const height = Math.max(1, Math.min(40, ms / 3600000 * 40)); return <rect key={i} x={i * 14} y={44 - height} width={10} height={height}><title>{`${24 - i} 小时前：${Math.round(ms / 60000)} 分钟`}</title></rect>; })}</svg></section>
        <TokenUsageCost usage={activity?.tokenUsage ?? null} price={activity?.priceEstimate ?? null} {...(activity?.pricePartial ? { detailsNote: '部分轮次没有已知价格' } : {})} />
        {error ? <ErrorMark message={error} onRetry={() => { setError(''); void store.loadDetail(thread.deviceId, thread.id, true).catch(error => setError(String(error))); }} /> : null}
      </div>
      <footer><button className="relay-button-primary" onClick={() => onOpen(thread.key)}>打开线程</button><button className="relay-button-secondary" disabled={!surface?.multiWindow} onClick={() => onOpen(thread.key, true)}><ExternalLink size={13} data-icon="open" />在新窗口打开</button>{!surface?.multiWindow ? <ErrorMark kind="unavailable" message="当前桌面外壳不支持多窗口。" /> : null}</footer>
    </div>
  </div>, document.body);
}
