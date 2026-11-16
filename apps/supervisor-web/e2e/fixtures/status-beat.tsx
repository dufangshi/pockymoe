import React from 'react';
import { createRoot } from 'react-dom/client';
import { ThreadMap } from '../../../../pockymoe-thread-ui/packages/thread-ui/src/components/threadMap/ThreadMap';
import { StatusMark } from '../../../../pockymoe-thread-ui/packages/thread-ui/src/components/StatusMark';
import { I18nProvider } from '../../../../pockymoe-thread-ui/packages/thread-ui/src/i18n';
import '../../src/lib/themePacks/anchor';
import { applyTheme } from '../../src/lib/themePacks/registry';
import '../../src/index.css';
import '../../../../pockymoe-thread-ui/packages/thread-ui/src/styles.css';
import { installStatusBeat } from '../../../../pockymoe-thread-ui/packages/thread-ui/src/statusBeat';

const releaseBeat = installStatusBeat();
Object.assign(window, { stopStatusFixtureBeat: releaseBeat });
const initialTheme = new URLSearchParams(location.search).get('theme') ?? 'classic';
const initialMode = new URLSearchParams(location.search).get('mode') ?? 'light';

// Synthetic, local-only data; no device, account, transport or harness is contacted.
const titles = ['Review runtime', 'Fix composer', 'Explore workspace', 'Check updates', 'Design settings', 'Measure status'];
const threads = titles.map((title, i) => ({ key: `fixture:${i}`, title, href: '#', agent: 'codex', running: true, turns: 20 + i, activeMs: 3_600_000, hourlyMs: Array.from({ length: 24 }, (_, h) => h % 3 * 120_000) }));
const statuses = ['running', 'agents-running', 'waiting', 'unread', 'idle', 'interrupted', 'failed', 'unknown'];

async function setTheme(theme: string, mode: string) {
  await applyTheme(theme);
  document.documentElement.dataset.themeEffective = mode;
  document.documentElement.style.colorScheme = mode;
  document.querySelectorAll<HTMLElement>('.thread-ui-shell').forEach(shell => { shell.dataset.themeEffective = mode; shell.dataset.themeMode = mode; });
}
Object.assign(window, { setStatusFixtureTheme: setTheme });

createRoot(document.getElementById('root')!).render(<I18nProvider><div className="thread-ui-shell" data-theme-effective={initialMode} data-theme-mode={initialMode}>
  <div className="matter-workbench fixture-workbench">
    <aside className="fixture-sidebar"><h2>Threads</h2>{statuses.map((status, i) => <div className="matter-thread-row" key={status}><StatusMark status={status} /><span>{titles[i] ?? status}</span></div>)}</aside>
    <main className="fixture-main"><div className="matter-tabs-row"><div className="matter-thread-tabs">{titles.slice(0, 5).map((title, i) => <div className="matter-thread-group" data-avatar={initialTheme === 'classic' ? undefined : ''} key={title}><a className="matter-group-tab" href="#" aria-current={i === 0 ? 'page' : undefined}><span className="matter-thread-avatar" data-agent="codex" /><StatusMark status="running" /><span>{title}</span></a></div>)}</div></div>
      <div className="fixture-map"><ThreadMap presentation="page" devices={[{ id: 'fixture', name: 'Test Mac', state: 'ready', workspaces: [{ id: 'workspace', label: 'Status performance', threads }] }]} onOpen={() => undefined} /></div>
      <div className="fixture-marks">{statuses.map(status => <label key={status}><StatusMark status={status} />{status}</label>)}</div>
    </main>
  </div>
</div></I18nProvider>);
const style = document.createElement('style');
style.textContent = `body{margin:0} .fixture-workbench{display:flex!important;height:100vh;min-height:0!important}.fixture-sidebar{width:210px;flex:none;padding:55px 12px 12px;border-right:1px solid var(--theme-border)}.fixture-sidebar h2{font-size:14px}.fixture-sidebar .matter-thread-row{display:flex;align-items:center;gap:10px;min-height:40px}.fixture-main{display:flex;flex:1;min-width:0;flex-direction:column;padding-top:38px}.fixture-map{flex:1;min-height:0}.fixture-map .thread-map{height:100%}.fixture-marks{display:flex;flex-wrap:wrap;gap:18px;padding:16px}.fixture-marks label{display:flex;gap:6px;align-items:center;font-size:12px}@media(max-width:639px){.fixture-sidebar{width:100px;padding:40px 6px 6px;font-size:10px}.fixture-sidebar .matter-thread-row{gap:5px}.fixture-main{padding-top:38px}.fixture-marks{gap:12px;padding:8px}}`;
document.head.append(style);
void setTheme(initialTheme, initialMode);
