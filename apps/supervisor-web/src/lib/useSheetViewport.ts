import { useEffect, useState, type CSSProperties } from 'react';

/**
 * Where a phone's bottom sheet fits: on the visible part of the page. A
 * software keyboard (and, on iOS, the scroll that keeps a field in view) only
 * shrinks the visual viewport, so a sheet anchored to the page bottom ends up
 * under the keyboard, its top above the screen. The sheet's CSS reads these
 * variables on phones (docs/ui-principles.md, floating layers).
 */
export function useSheetViewport(enabled = true): CSSProperties | undefined {
  const [place, setPlace] = useState<{ bottom: number; height: number } | null>(null);
  useEffect(() => {
    const viewport = typeof window === 'undefined' ? null : window.visualViewport;
    if (!enabled || !viewport) return;
    const update = () => {
      const height = Math.round(viewport.height);
      const bottom = Math.max(0, Math.round(window.innerHeight - viewport.offsetTop - viewport.height));
      setPlace((current) => current?.bottom === bottom && current.height === height ? current : { bottom, height });
    };
    update();
    viewport.addEventListener('resize', update);
    viewport.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    return () => {
      viewport.removeEventListener('resize', update);
      viewport.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
    };
  }, [enabled]);
  return place
    ? ({ '--sheet-bottom': `${place.bottom}px`, '--sheet-height': `${place.height}px` } as CSSProperties)
    : undefined;
}
