import { useEffect, useState } from 'react';

/** Reacts to a media query; false during the first server-less render. */
export function useMediaQuery(queryText: string) {
  const get = () => typeof window !== 'undefined' && typeof window.matchMedia === 'function' && window.matchMedia(queryText).matches;
  const [matches, setMatches] = useState(get);
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(queryText);
    const update = () => setMatches(media.matches);
    update();
    media.addEventListener?.('change', update);
    return () => media.removeEventListener?.('change', update);
  }, [queryText]);
  return matches;
}
