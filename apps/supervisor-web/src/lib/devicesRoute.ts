import { hostSurface } from '../desktop/surface';

// Decide once per document, after host capability discovery. Resizing a mounted
// route never changes its destination. Phones have no dedicated Devices page.
const phoneWidthOnLoad = typeof window !== 'undefined' && window.matchMedia('(max-width: 639px)').matches;
export function devicesHref(query = '') {
  const surface = hostSurface();
  return `${surface === 'mobile' || (surface !== 'desktop' && phoneWidthOnLoad) ? '/relay-devices' : '/desktop/devices'}${query}`;
}
