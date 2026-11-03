import { Navigate, useLocation } from 'react-router-dom';
import { devicesHref } from '../lib/devicesRoute';
import { RelayDevicesPage } from './RelayDevicesPage';

/** Shipped links remain valid; the phone page is deliberately retained. */
export function DevicesEntryRoute() {
  const location = useLocation();
  const destination = devicesHref();
  return destination === '/relay-devices' ? <RelayDevicesPage /> : <Navigate replace to={destination + location.search} />;
}
