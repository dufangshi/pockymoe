import { useEffect, useState, type ReactNode } from 'react';
import { fetchRelaySession } from '../lib/api';
import type { HostCapabilities } from './host';
import { DesktopIndexStore, DesktopStoreContext } from './data';
import { DesktopShell } from './DesktopShell';
import { installDetailCache } from './detailCache';

export default function DesktopEntry({ children }: { children: ReactNode }) {
  const [ready, setReady] = useState<{ store: DesktopIndexStore; capabilities: HostCapabilities } | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let cancelled = false; let store: DesktopIndexStore | undefined; let uninstall: (() => void) | undefined;
    void (async () => {
      const [session, capabilities] = await Promise.all([fetchRelaySession(), window.remoteCodexHost!.capabilities()]);
      if (!session.authenticated || !session.user) throw new Error('请重新登录你的 Relay 账号。');
      store = new DesktopIndexStore(session.user.id);
      // IndexedDB is asynchronous. Hydrate before mounting DesktopShell, so its
      // first committed frame already contains a warm tree instead of a spinner.
      await store.hydrate();
      if (cancelled) return;
      uninstall = installDetailCache(store);
      performance.mark('rc-desktop-shell-mount');
      setReady({ store, capabilities }); store.start();
    })().catch(error => { if (!cancelled) setError(error instanceof Error ? error.message : '无法打开桌面'); });
    return () => { cancelled = true; store?.stop(); uninstall?.(); };
  }, []);
  if (!ready) return error ? <div role="alert" style={{ padding: 32 }}>{error}<button onClick={() => location.assign('/relay-portal')}>登录</button></div> : null;
  return <DesktopStoreContext.Provider value={ready.store}><DesktopShell capabilities={ready.capabilities}>{children}</DesktopShell></DesktopStoreContext.Provider>;
}
