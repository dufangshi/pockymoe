import React from 'react';
import { createRoot } from 'react-dom/client';
// Deliberately consume the published entry and its injected styles. Importing
// source CSS here would conceal a missing stylesheet in the tracked bundle.
import { I18nProvider, StatusMark } from '@pockymoe/thread-ui';
import { Bot, LoaderCircle, Hourglass, Layers } from 'lucide-react';
import '../../src/lib/themePacks/anchor';
import { applyTheme } from '../../src/lib/themePacks/registry';
import '../../src/index.css';
import '../../src/native-subagents.css';
import '../../src/components/environments/environments.css';
import '../../src/components/ResourceSettings.css';
import '../../src/pages/share-pending.css';
import { BuiltLifecycle } from './built-lifecycle';
import { BuiltLibraries } from './built-libraries';

void (async () => {
const params = new URLSearchParams(location.search);
const theme = params.get('theme') ?? 'classic', mode = params.get('mode') ?? 'light';
await applyTheme(theme);
document.documentElement.dataset.themeEffective = mode;
document.documentElement.style.colorScheme = mode;
const row = (id: string, label: string, content: React.ReactNode) => params.has('empty') ? null : <section className="audit-row" data-audit={id}><small>{label}</small><div>{content}</div></section>;
const tab = (status: string) => <div className="matter-thread-group" data-avatar={theme === 'classic' ? undefined : ''}><a className="matter-group-tab"><span className="matter-thread-avatar" data-agent="codex" /><StatusMark status={status} /><span>{status}</span></a></div>;
createRoot(document.getElementById('root')!).render(<I18nProvider><div className="thread-ui-shell" data-theme-effective={mode} data-theme-mode={mode}>{params.has('libraries') ? <BuiltLibraries /> : params.has('lifecycle') ? <BuiltLifecycle /> : <div className="audit-grid matter-workbench">
  {params.has('idle') && <aside className="fixture-sidebar">{Array.from({ length: params.get('empty') === '1' ? 0 : 30 }, (_, i) => <div className="matter-thread-row" key={i}><StatusMark status="running" /><span>Thread {i}</span></div>)}</aside>}
  <h1>{theme} · {mode} · built bundle</h1>
  {row('status', 'Sidebar states', <aside className="matter-sidebar audit-sidebar">{['running', 'agents-running', 'waiting', 'unread', 'idle', 'interrupted', 'failed', 'unknown'].map(s => <label className="matter-thread-row" key={s}><StatusMark status={s} />{s}</label>)}</aside>)}
  {row('tabs', 'Tab strip: running / agents running', <div className="matter-workbench audit-tab-host"><div className="matter-tabs-row"><div className="matter-tab-scroll"><div className="matter-thread-tabs">{tab('running')}{tab('agents-running')}</div></div></div></div>)}
  {row('badge', 'Agent badge: running / idle', <><button className="matter-group-toggle is-badge" data-running=""><Bot />3</button><button className="matter-group-toggle is-badge"><Bot />3</button></>)}
  {row('map', 'Map: tile / dense / section / dot', <div className="thread-map" data-presentation="page"><div className="thread-map-tile" data-running="">Running tile</div><div className="thread-map-dense-tile" data-running="">Dense tile</div><div className="thread-map-section" data-running="">Running section</div><i className="thread-map-running-dot" /></div>)}
  {row('transcript', 'Live turn / log spinner / room spinner', <><div className="thread-turn-head" data-active=""><i className="thread-turn-head-dot" /><span>Working</span></div><i className="tlog-spinner" /><LoaderCircle className="thread-room-running" /></>)}
  {row('agents', 'Background agents / waiting hourglass', <><span className="thread-background-agent-status"><Bot />2 agents running</span><span className="thread-wait-mark"><Hourglass />Sleeping</span></>)}
  {row('thought', 'Thought and operation text', <><div className="tlog-current is-thought"><div className="tlog-row"><span className="tlog-title">Thinking through the next change</span></div></div><div className="is-running-batch"><span className="thread-graph-history-group-verb">Reading</span> <span className="thread-graph-history-group-description">the workspace files</span></div></>)}
  {row('phone', 'Phone running dot (phone width)', <span className="matter-running-icon"><Layers /><i /></span>)}
  {row('plum', 'Plum active transcript / small avatar', <div className="thread-graph-turn" data-turn-active="true"><div className="thread-transcript-agent"><span className="matter-thread-avatar" data-agent="codex" /></div><div className="thread-graph-turn-footer"><span className="matter-thread-avatar" data-agent="codex" /></div></div>)}
  {row('pending', 'Connection / reconnect / share pending', <><span className="thread-connection-pending">Connecting…</span><span className="thread-live-connection-reconnecting">Reconnecting…</span><span className="share-pending-status"><i className="share-pending-pulse" />Waiting for approval</span></>)}
  {row('host', 'Native agents / environment / resource job', <><span className="native-agents-running-count"><Bot />2</span><span className="env-state" data-state="starting"><i className="env-state-mark" />Starting</span><div className="pm-res-job-bar is-indeterminate"><span /></div></>)}
  {!params.has('idle') && row('busy', 'Generic busy button / pulse', <><button><LoaderCircle className="request-spin" />Saving…</button><span className="request-pulse">Loading…</span></>)}
</div>}</div></I18nProvider>);
const style = document.createElement('style');
// Layout only; no status appearance, animation or theme tokens are supplied here.
style.textContent = `body{margin:0}.audit-grid{display:block!important;grid-template-columns:none!important;height:auto!important;overflow:visible!important;padding:18px;background:var(--theme-bg)!important;color:var(--theme-fg)!important;min-height:100vh}h1{font:600 16px system-ui;margin:0 0 12px}.audit-row{padding:10px 0;border-bottom:1px solid var(--theme-border);min-height:66px}.audit-row>small{display:block;font:11px system-ui;color:var(--theme-fg-muted);margin-bottom:10px}.audit-row .audit-sidebar{display:flex!important;flex-direction:row!important;position:static!important;transform:none!important;visibility:visible!important;flex-wrap:wrap;gap:10px;width:100%;height:auto;overflow:visible;padding:8px}.audit-row .audit-tab-host{display:block!important;height:auto!important;min-height:0!important;width:100%;overflow:visible}.audit-row .matter-tabs-row{width:100%}.audit-row .matter-tab-scroll{min-width:0;max-width:100%}.audit-row>div,.audit-row label{display:flex;align-items:center;gap:15px;flex-wrap:wrap}.audit-row label{gap:5px;font-size:11px}.audit-row svg{width:16px;height:16px}.audit-row .thread-map{display:flex;flex-direction:row;height:44px;gap:12px;width:100%}.audit-row .thread-map :is(.thread-map-tile,.thread-map-dense-tile,.thread-map-section){position:relative;inset:auto;display:grid;place-items:center;min-height:36px;width:110px;padding:4px;font-size:11px}.audit-row .thread-map-running-dot{flex:none}.audit-row .thread-turn-head{display:flex;align-items:center;gap:10px;padding:0}.audit-row .thread-graph-turn{display:flex;align-items:center;gap:18px}.audit-row .pm-res-job-bar{width:120px}.audit-row button{display:inline-flex;align-items:center;gap:6px}.audit-row .tlog-row{padding:0}.audit-row .matter-thread-group{flex:none}.audit-row .matter-group-tab{min-width:130px}@media(max-width:639px){.audit-grid{padding:12px}.audit-row{min-height:75px}.audit-row>div{gap:12px}.audit-row .thread-map{gap:4px}.audit-row .thread-map :is(.thread-map-tile,.thread-map-dense-tile,.thread-map-section){width:95px}}`;
document.head.append(style);
document.documentElement.dataset.auditReady = 'true';

})();
