import { ExternalLink } from 'lucide-react';
import { useEffect, useState } from 'react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import './OpenFullPageButton.css';

/** Inside a Treer Agent tab, opens this same page as a full browser tab. Treer
 *  frames allow it with `allow-popups`; where they do not, say where Treer's own
 *  entry is. A page that is already top-level shows nothing. */
export function OpenFullPageButton() {
  useI18n();
  const [blocked, setBlocked] = useState(false);
  useEffect(() => {
    if (!blocked) return;
    const timer = window.setTimeout(() => setBlocked(false), 8000);
    return () => window.clearTimeout(timer);
  }, [blocked]);
  if (window.self === window.top) return null;
  const label = translate('workbench.openFullPage');
  return (
    <span className="pm-open-full-page">
      <button type="button" aria-label={label} title={label} onClick={() => {
        // Not `noopener`: it always returns null, and null is how a blocked tab shows.
        const opened = window.open(window.location.href, '_blank');
        if (opened) opened.opener = null;
        else setBlocked(true);
      }}>
        <ExternalLink data-icon="external" aria-hidden="true" />
      </button>
      {blocked && <span role="status" className="pm-open-full-page-hint">{translate('workbench.openFullPageBlocked')}</span>}
    </span>
  );
}
