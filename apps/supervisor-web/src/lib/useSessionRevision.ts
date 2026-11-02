import { useSyncExternalStore } from 'react';
import { sessionRevision, subscribeSessionActor } from './sessionActor';
export const useSessionRevision = () => useSyncExternalStore(subscribeSessionActor, sessionRevision, sessionRevision);
