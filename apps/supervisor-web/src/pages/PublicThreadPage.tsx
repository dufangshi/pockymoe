import { relayUrl } from '../lib/relayOrigin';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useState } from 'react';
import { decryptPublicSnapshot, PublicLinkKeyError } from '../lib/publicLinkCrypto';
import { useParams } from 'react-router-dom';
import {
  PublicTranscript,
  type PublicTranscriptSnapshot,
} from '@pockymoe/thread-ui';

export function PublicThreadPage() {
  useI18n();
  const { id } = useParams();
  const [snapshot, setSnapshot] = useState<PublicTranscriptSnapshot | null>(
    null,
  );
  const [fragment, setFragment] = useState(location.hash);
  useEffect(() => {
    const changed = () => setFragment(location.hash);
    window.addEventListener('hashchange', changed);
    return () => window.removeEventListener('hashchange', changed);
  }, []);
  const [error, setError] = useState('');
  useEffect(() => {
    const robots = document.createElement('meta');
    robots.name = 'robots';
    robots.content = 'noindex, nofollow, noarchive';
    document.head.appendChild(robots);
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    setSnapshot(null);
    setError('');
    async function refresh() {
      try {
        const response = await fetch(relayUrl(`relay/public-links/${encodeURIComponent(id ?? '')}`), {
          signal: controller.signal,
          // Public links never identify the visitor, on either origin layout.
          credentials: 'omit',
          cache: 'no-store',
        });
        if (!response.ok)
          throw new Error(
            response.status === 404
              ? translate("sharing.thisShareLinkIsUnavailableOrHas")
              : translate("sharing.unableToLoadThisSharedThread"),
          );
        const value = await decryptPublicSnapshot(await response.json(), fragment);
        if (controller.signal.aborted) return;
        setSnapshot(value);
        setError('');
        if (value.live) timer = setTimeout(refresh, 5000);
      } catch (error) {
        if (!controller.signal.aborted) {
          setSnapshot(null);
          setError(error instanceof PublicLinkKeyError
            ? translate(error.reason === 'missing' ? "sharing.publicLinkKeyMissing" : "sharing.publicLinkKeyWrong")
            : error instanceof Error ? error.message : translate("sharing.unableToLoadThisSharedThread"));
          if (!(error instanceof PublicLinkKeyError)) timer = setTimeout(refresh, 5000);
        }
      }
    }
    void refresh();
    return () => {
      controller.abort();
      clearTimeout(timer);
      robots.remove();
    };
  }, [id, fragment]);
  useEffect(() => {
    if (!snapshot) return;
    const previous = document.documentElement.getAttribute(
      'data-theme-effective',
    );
    document.documentElement.setAttribute(
      'data-theme-effective',
      snapshot.theme ?? 'dark',
    );
    return () => {
      if (previous)
        document.documentElement.setAttribute('data-theme-effective', previous);
      else document.documentElement.removeAttribute('data-theme-effective');
    };
  }, [snapshot]);
  return (
    <div className="thread-ui-shell public-thread-page min-h-screen bg-[var(--theme-bg)] text-[var(--theme-fg)]">
      <div className="px-4 py-3"><LanguageSwitcher /></div>
      {snapshot ? (
        <PublicTranscript snapshot={snapshot} />
      ) : (
        <p role="status" className="p-8 text-center">
          {error || translate("sharing.loadingSharedThread")}
        </p>
      )}
    </div>
  );
}
