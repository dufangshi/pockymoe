// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { useOptimisticAttachmentPreviews } from './useOptimisticAttachmentPreviews';
it('keeps a just-sent photo alive through acknowledgement/live metadata, releases on settlement and remount', () => {
  const revoke = vi.fn(); vi.stubGlobal('URL', Object.assign(URL, { revokeObjectURL: revoke }));
  const host = document.createElement('div'); const root = createRoot(host);
  const previews = [{ url: 'blob:just-sent' }];
  function Prompt({ turn }: { turn: { id: string; previews: typeof previews } | null }) {
    useOptimisticAttachmentPreviews(turn?.previews);
    return turn ? <img src={turn.previews[0]!.url} /> : <img src="/durable.png" />;
  }
  try {
    act(() => root.render(<Prompt turn={{ id: 'optimistic', previews }} />));
    act(() => root.render(<Prompt turn={{ id: 'server-ack', previews }} />));
    act(() => root.render(<Prompt turn={{ id: 'live-output', previews }} />));
    expect(revoke).not.toHaveBeenCalled();
    act(() => root.render(<Prompt turn={null} />));
    expect(revoke).toHaveBeenCalledExactlyOnceWith('blob:just-sent');
    expect(host.querySelector('img')!.getAttribute('src')).toBe('/durable.png');
    act(() => root.render(<Prompt key="other-thread" turn={null} />));
    expect(host.querySelector('img')!.getAttribute('src')).toBe('/durable.png');
  } finally { act(() => root.unmount()); vi.unstubAllGlobals(); }
});
