import '../lib/themePacks/anchor';
import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ThreadTimeline, ImageRecoveryProvider } from '@pockymoe/thread-ui';
import { I18nProvider, initializeI18n } from '@pockymoe/thread-ui/i18n';
import { useOptimisticAttachmentPreviews } from '../pages/useOptimisticAttachmentPreviews';
import { initializeThemePacks, THEME_PRESET_KEY } from '../lib/themePacks';
import '../index.css';

const query = new URLSearchParams(location.search);
const mode = query.get('mode') ?? 'light';
localStorage.setItem(THEME_PRESET_KEY, query.get('theme') ?? 'classic');
localStorage.setItem('remote-codex.locale', 'en');
document.documentElement.classList.toggle('dark', mode === 'dark');
document.documentElement.dataset.themeEffective = mode;
document.documentElement.dataset.themeMode = mode;
const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="180" height="110"><rect width="180" height="110" fill="#879b82"/><circle cx="90" cy="55" r="30" fill="#ead6ad"/></svg>';
type Turn = { status: string; previews: { url: string }[] };
function Preview() {
  const [turn, setTurn] = useState<Turn | null>(null);
  const [visible, setVisible] = useState(true);
  useOptimisticAttachmentPreviews(turn?.previews);
  useEffect(() => {
    if (!query.has('legacy')) return;
    // Exact pre-fix ownership rule, retained only to reproduce the regression.
    return () => { turn?.previews.forEach(preview => URL.revokeObjectURL(preview.url)); };
  }, [turn]);
  const sentItem = { id: 'prompt', kind: 'userMessage' as const, text: '[PHOTO image59.png]', attachmentPreviewUrls: turn ? { 'image59.png': turn.previews[0]!.url } : {} };
  return <ImageRecoveryProvider value={src => src}><div className={`thread-ui-shell ${mode === 'dark' ? 'thread-ui-theme-dark dark' : ''}`} data-theme-effective={mode} style={{ padding: 24, minHeight: '100vh', background: 'var(--theme-bg)', color: 'var(--theme-fg)' }}>
    <h1>Chat images</h1>
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 12, margin: '20px 0' }}>
      <button onClick={() => setTurn({ status: 'sending', previews: [{ url: URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' })) }] })}>Send image</button>
      <button onClick={() => setTurn(current => current ? { ...current, status: 'running' } : null)}>Acknowledge</button>
      <button onClick={() => setTurn(null)}>Settle</button>
      <button onClick={() => setVisible(current => !current)}>Switch thread</button>
      <button onClick={() => window.dispatchEvent(new Event('pockymoe-images-reconnected'))}>Reconnect</button>
    </div>
    <p role="status">{turn?.status ?? 'settled'}</p>
    {visible && <section aria-label="Sent image" style={{ margin: '20px 0' }}>
      <ThreadTimeline threadId="fixture" turns={[]} liveOutput="" optimisticTurn={{ id: 'sent', startedAt: '2026-11-03T12:00:00Z', status: 'inProgress', error: null, items: [sentItem] }} adapter={{ getImageAssetUrl: () => '/chat-image-fixture.png' }} />
    </section>}
    <section aria-label="Inline image"><ThreadTimeline turns={[]} liveOutput="" optimisticTurn={{ id: 'reply', startedAt: '2026-11-03T12:00:00Z', status: 'inProgress', error: null, items: [{ id: 'reply-image', kind: 'agentMessage', text: '![Chat illustration](/chat-image-inline.png)' }] }} /></section>
  </div></ImageRecoveryProvider>;
}
initializeI18n();
void initializeThemePacks({ timeoutMs: 1500 }).finally(() => createRoot(document.getElementById('root')!).render(<I18nProvider><Preview /></I18nProvider>));
