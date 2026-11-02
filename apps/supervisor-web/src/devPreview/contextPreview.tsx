// Before the app's modules: fixes the theme pack's place in the cascade.
import '../lib/themePacks/anchor';
import { I18nProvider, initializeI18n } from '@pockymoe/thread-ui/i18n';
import { ThreadComposer, type PromptAttachmentUpload, type ThreadContextInspector } from '@pockymoe/thread-ui';
import { StrictMode, useEffect, useMemo, useState } from 'react';
import ReactDOM from 'react-dom/client';
import { MemoryRouter } from 'react-router-dom';
import '../index.css';
import { ContextInspectionSetting } from '../components/HarnessResourceSettings';
import { fetchThreadContext } from '../lib/api';
import { THEME_PRESET_KEY, initializeThemePacks } from '../lib/themePacks';
import { contextPreviewScenarios as scenarios } from './contextFixtures';

/** The real composer, panel, setting and API client against a stubbed Supervisor:
    /context-preview.html?scenario=codex&theme=terminal&mode=dark&lang=zh-CN&open=1
    Add chrome=0 to hide the switches for screenshots. Development only. */
const query = new URLSearchParams(location.search);
const scenario = scenarios.find(entry => entry.id === query.get('scenario')) ?? scenarios[0]!;
const theme = query.get('theme') ?? 'classic';
const mode = query.get('mode') === 'dark' ? 'dark' : 'light';
const language = query.get('lang') ?? 'zh-CN';
const THREAD_ID = 'preview-thread';
const compactAvailable = query.get('compact') !== '0' && scenario.model.agent !== 'grok';

localStorage.setItem(THEME_PRESET_KEY, theme);
localStorage.setItem('remote-codex.locale', language);
const root = document.documentElement;
root.classList.toggle('dark', mode === 'dark');
root.dataset.themeEffective = mode;
root.dataset.themeMode = mode;

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
let settingEnabled = scenario.settingsEnabled ?? false;
const realFetch = window.fetch.bind(window);
window.fetch = async (input, init) => {
  const url = new URL(typeof input === 'string' ? input : input instanceof URL ? input.href : input.url, location.href);
  if (url.pathname.endsWith('api/context-inspection/settings')) {
    if (init?.method === 'PUT') settingEnabled = (JSON.parse(String(init.body)) as { enabled: boolean }).enabled;
    return json({ enabled: settingEnabled });
  }
  if (url.pathname.endsWith(`api/threads/${THREAD_ID}/context`)) {
    const detail = url.searchParams.get('detail') === 'full' && scenario.full ? scenario.full : scenario.summary;
    await new Promise(resolve => setTimeout(resolve, 160));
    if (detail === 'pending') return new Promise<Response>(() => {});
    if (detail === 'error') return json({ code: 'context_read_failed', message: 'The harness did not answer in time.' }, 500);
    return json(detail);
  }
  return realFetch(input, init);
};

function Preview() {
  const [compacting, setCompacting] = useState(false);
  const [draft, setDraft] = useState<{ prompt: string; attachments: PromptAttachmentUpload[] }>({ prompt: '', attachments: [] });
  const inspector = useMemo<ThreadContextInspector>(() => ({
    load: options => fetchThreadContext(THREAD_ID, options),
    openSettings: () => document.querySelector('[data-testid="context-inspection-setting"]')?.scrollIntoView({ block: 'center' }),
  }), []);
  useEffect(() => {
    if (query.get('open') !== '1') return;
    // Editing unfolds the composer; then the same tap a reader would make.
    const timer = window.setInterval(() => {
      const editor = document.querySelector<HTMLElement>('[role="textbox"]');
      const meter = document.querySelector<HTMLElement>('[data-testid="composer-context-meter"]');
      if (!editor || !meter) return;
      editor.focus();
      if (meter.offsetParent === null) return;
      window.clearInterval(timer);
      meter.click();
    }, 80);
    return () => window.clearInterval(timer);
  }, []);
  const link = (patch: Record<string, string>) => {
    const next = new URLSearchParams(location.search);
    Object.entries(patch).forEach(([key, value]) => next.set(key, value));
    return `?${next}`;
  };
  return (
    <div className={`thread-ui-shell ${mode === 'dark' ? 'thread-ui-theme-dark dark' : ''}`} data-theme-effective={mode} data-theme-mode={mode}
      style={{ minHeight: '100dvh', display: 'flex', flexDirection: 'column', background: 'var(--theme-bg)', color: 'var(--theme-fg)' }}>
      {query.get('chrome') === '0' ? null : (
        <nav style={{ display: 'flex', flexWrap: 'wrap', gap: 6, padding: 10, fontSize: 12, borderBottom: '1px solid var(--theme-border)' }}>
          {scenarios.map(entry => <a key={entry.id} href={link({ scenario: entry.id })} title={entry.title} style={{ fontWeight: entry === scenario ? 700 : 400, textDecoration: 'underline' }}>{entry.id}</a>)}
          <span>·</span>
          {['classic', 'plum-pocket', 'terminal'].map(id => <a key={id} href={link({ theme: id })} style={{ fontWeight: id === theme ? 700 : 400, textDecoration: 'underline' }}>{id}</a>)}
          <span>·</span>
          {['light', 'dark'].map(id => <a key={id} href={link({ mode: id })} style={{ fontWeight: id === mode ? 700 : 400, textDecoration: 'underline' }}>{id}</a>)}
          <span>·</span>
          {['zh-CN', 'en'].map(id => <a key={id} href={link({ lang: id })} style={{ fontWeight: id === language ? 700 : 400, textDecoration: 'underline' }}>{id}</a>)}
        </nav>
      )}
      <main style={{ flex: 1, padding: 16, overflow: 'auto' }}>
        <p style={{ fontWeight: 600 }}>{scenario.title}</p>
        {query.get('chrome') === '0' ? null : (
          <MemoryRouter><div className="pm-resources" style={{ maxWidth: 620, marginTop: 12 }}><ContextInspectionSetting deviceId={null} /></div></MemoryRouter>
        )}
      </main>
      <div style={{ width: '100%', maxWidth: 860, margin: '0 auto', padding: 8 }} data-testid="chat-composer">
        <ThreadComposer
          sendShortcut="ctrlEnter"
          draftPrompt={draft.prompt}
          draftAttachments={draft.attachments}
          onDraftChange={setDraft}
          busy={false}
          settingsBusy={false}
          compactBusy={compacting}
          error={null}
          model={scenario.model.id}
          agentLabel={scenario.model.agent}
          reasoningEffort="high"
          fastMode={false}
          collaborationMode="default"
          modelOptions={[{ id: scenario.model.id, model: scenario.model.id, displayName: scenario.model.name, description: '', isDefault: true, hidden: false, defaultReasoningEffort: 'high', supportedReasoningEfforts: [{ reasoningEffort: 'medium', description: '' }, { reasoningEffort: 'high', description: '' }] }]}
          contextUsage={scenario.usage}
          contextInspector={inspector}
          capabilities={{ controls: {}, turns: { compact: compactAvailable }, branching: {}, management: {} } as never}
          toolboxItems={compactAvailable ? [{ action: 'compact', command: '/compact', label: 'Compact' }] : []}
          threadConnected
          shellAvailable={false}
          activeView="chat"
          followTail
          onCompact={() => { setCompacting(true); window.setTimeout(() => setCompacting(false), 1200); }}
          onSubmit={() => undefined}
        />
      </div>
    </div>
  );
}

initializeI18n();
void initializeThemePacks({ timeoutMs: 1500 }).finally(() => {
  ReactDOM.createRoot(document.getElementById('root')!).render(<StrictMode><I18nProvider><Preview /></I18nProvider></StrictMode>);
});
