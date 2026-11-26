import { createContext } from 'react';
import type { DesktopIndexStore } from './store';

/** Context-only import: navigation consumers do not initialize data adapters. */
export const DesktopStoreContext = createContext<DesktopIndexStore | null>(null);

/** The mobile host's account index (MobileActivity), offered to the pages that
 * list devices so a phone keeps one index per account, not one per page. */
export const MobileIndexContext = createContext<DesktopIndexStore | null>(null);
