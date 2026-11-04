import { createContext } from 'react';
import type { DesktopIndexStore } from './store';

/** Context-only import: navigation consumers do not initialize data adapters. */
export const DesktopStoreContext = createContext<DesktopIndexStore | null>(null);
