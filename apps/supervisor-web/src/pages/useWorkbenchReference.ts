import { useHostScope } from '../lib/hostCapabilities';
import { useCallback, useEffect, useRef, useState } from 'react';
import type {
  AgentProviderCapabilitiesDto, ModelOptionDto, RelayEffectiveAccessDto,
  ThreadDetailDto, ThreadEventEnvelope, UpdateThreadSettingsInput,
} from '@pockymoe/shared';
import {
  isInvisibleThreadError, connectSupervisorEvents, cancelPendingSteer, fetchRelayAccess, fetchThreadCapabilitySnapshot,
  fetchThreadDetail, fetchThreadModels, interruptThread, relayModeActive,
  respondToThreadRequest, resumeThread, sendThreadPrompt, steerPendingPrompt,
  steerSubmittedPrompt, updateThreadSettings, type SendThreadPromptRequestInput,
} from '../lib/api';
import { translate as t } from '@pockymoe/thread-ui/i18n';
import { useThreadView } from '../lib/threadView';
import { createClientRequestId, prependTurns } from './threadDetailModel';
import { useScopedState } from './useScopedState';

/** A second conversation controller: subscribes to its own device, never auto-resumes,
 * acknowledges reads, mounts device management, or changes the host route. */
/** `displayed`: the split shows this conversation, so a controllable one counts as viewed. */
export function useWorkbenchReference(device: string, threadId: string | null, hostDevice = device, displayed = false) {
  const hostScope = useHostScope();
  const [unavailable, setUnavailable] = useScopedState(`${device}:${threadId ?? ''}`, false);
  const source = `${device}:${threadId ?? ''}`;
  const [detail, setDetail] = useScopedState<ThreadDetailDto | null>(source, null);
  const [error, setError] = useScopedState<string | null>(source, null);
  const [loadingEarlier, setLoadingEarlier] = useScopedState(source, false);
  const [busy, setBusy] = useScopedState(source, false);
  const [respondingRequestId, setRespondingRequestId] = useScopedState<string | null>(source, null);
  const [access, setAccess] = useScopedState<RelayEffectiveAccessDto | null>(source, null);
  const [accessReady, setAccessReady] = useScopedState(source, false);
  const [capabilities, setCapabilities] = useScopedState<AgentProviderCapabilitiesDto | null>(source, null);
  const [models, setModels] = useScopedState<ModelOptionDto[]>(source, []);
  const requestOwner = useRef({ source, mutation: false, version: 0, earlier: false });
  if (requestOwner.current.source !== source)
    requestOwner.current = { source, mutation: false, version: 0, earlier: false };
  const owner = requestOwner.current;
  const isCurrent = () => requestOwner.current === owner;
  const detailRef = useRef(detail);
  detailRef.current = detail;
  const refreshRef = useRef<() => void>(() => {});
  const [revision, setRevision] = useState(0);
  const relay = device !== 'local' && relayModeActive();
  const canControl = !relay || (accessReady && (access?.kind === 'owner' || access?.threadAccess === 'control'));
  const disabledReason = !canControl
    ? t(accessReady ? 'workbench.thisSharedSessionIsViewOnly' : 'workbench.checkingRelayPermissions')
    : detail?.workspacePathStatus === 'missing'
      ? t('workbench.restoreThisWorkspacePathOnTheCurrent') : null;

  const mergeDetail = useCallback((next: ThreadDetailDto) => {
    setDetail(current => current ? {
      ...next,
      turns: prependTurns(next.turns, current.turns.filter(turn => !next.turns.some(t => t.id === turn.id))),
    } : next);
  }, [setDetail]);
  useEffect(() => {
    if (!threadId) return;
    let alive = true;
    void Promise.allSettled([
      relay ? fetchRelayAccess({ deviceId: device, threadId }) : Promise.resolve(null),
      // Reading a reference must not start its harness.
      fetchThreadCapabilitySnapshot(threadId, device, { load: false }), fetchThreadModels(threadId, device),
    ]).then(([acl, caps, model]) => {
      if (!alive || !isCurrent()) return;
      if (acl.status === 'fulfilled') { setAccess(acl.value); setAccessReady(true); }
      else setError(acl.reason instanceof Error ? acl.reason.message : String(acl.reason));
      if (caps.status === 'fulfilled') setCapabilities(caps.value.effectiveCapabilities ?? null);
      if (model.status === 'fulfilled') setModels(model.value);
    });
    return () => { alive = false; };
  }, [threadId, device, relay, owner, revision, setAccess, setAccessReady, setCapabilities, setModels, setError]);
  useEffect(() => {
    if (!threadId) return;
    let alive = true;
    let pending = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    async function refresh() {
      if (!alive || pending || owner.mutation) return;
      pending = true;
      const version = owner.version;
      try {
        const next = await fetchThreadDetail(threadId!, { limit: 3 }, device);
        if (alive && isCurrent() && version === owner.version) mergeDetail(next);
      } catch (caught) {
        if (alive && isCurrent()) {
          if (hostScope && isInvisibleThreadError(caught)) setUnavailable(true);
          else setError(caught instanceof Error ? caught.message : String(caught));
        }
      } finally { pending = false; }
    }
    refreshRef.current = () => {
      if (!timer) timer = setTimeout(() => { timer = undefined; void refresh(); }, 100);
    };
    void refresh();
    const interval = setInterval(() => {
      if (document.visibilityState !== 'hidden') void refresh();
    }, 3000);
    return () => {
      alive = false; clearInterval(interval); clearTimeout(timer);
      refreshRef.current = () => {};
    };
  }, [threadId, owner, revision, mergeDetail, setError, device, hostScope, setUnavailable]);
  const onEvent = useCallback((event: ThreadEventEnvelope) => {
    if (device === hostDevice && event.threadId === threadId) refreshRef.current();
  }, [threadId, device, hostDevice]);
  // A reference on the host device is reported on the host page's socket.
  const viewSocketRef = useRef<WebSocket | null>(null);
  const threadView = useThreadView(viewSocketRef, threadId && device !== hostDevice && displayed && canControl ? [threadId] : []);
  useEffect(() => {
    if (!threadId || device === hostDevice) return;
    const socket = connectSupervisorEvents(event => {
      if (event.threadId === threadId) refreshRef.current();
    }, { deviceId: device === 'local' ? null : device, threadId });
    viewSocketRef.current = socket;
    socket.addEventListener('open', () => threadView.socketOpened());
    return () => {
      viewSocketRef.current = null;
      socket.close();
    };
  }, [device, hostDevice, threadId, threadView]);

  async function mutate(action: () => Promise<unknown>) {
    if (!threadId || !isCurrent() || !canControl || owner.mutation) return false;
    owner.mutation = true; owner.version += 1;
    setBusy(true); setError(null);
    try {
      await action();
      if (isCurrent()) {
        try { mergeDetail(await fetchThreadDetail(threadId, { limit: 3 }, device)); }
        catch (caught) { if (isCurrent()) setError(caught instanceof Error ? caught.message : String(caught)); }
      }
      return true;
    } catch (caught) {
      if (isCurrent()) setError(caught instanceof Error ? caught.message : String(caught));
      return false;
    } finally {
      owner.mutation = false;
      if (isCurrent()) { setBusy(false); refreshRef.current(); }
    }
  }
  async function send(input: SendThreadPromptRequestInput) {
    if (disabledReason || !threadId) return false;
    if ((input.attachments?.length ?? 0) > 10) { setError(t('workbench.aPromptCanIncludeAtMost10')); return false; }
    if (input.delivery === 'steer' && capabilities?.turns.steer !== true) {
      setError(t('workbench.thisBackendDoesNotSupportSteeringAn')); return false;
    }
    // Capture every target before awaiting; switching/closing never retargets requests.
    const target = threadId;
    const current = detailRef.current;
    const clientRequestId = createClientRequestId();
    return mutate(async () => {
      if (!current) throw new Error(t('workbench.threadDetailIsStillLoading'));
      if (!current.thread.isLoaded && current.thread.status !== 'recovering')
        await resumeThread(target, current.thread.model ? { model: current.thread.model } : {}, device);
      await sendThreadPrompt(target, {
        prompt: input.prompt, ...(input.attachments?.length ? { attachments: input.attachments } : {}), clientRequestId,
        ...(current.thread.model ? { model: current.thread.model } : {}),
        ...(current.thread.reasoningEffort ? { reasoningEffort: current.thread.reasoningEffort } : {}),
        collaborationMode: current.thread.collaborationMode,
      }, device);
      if (input.delivery === 'steer' && current.thread.activeTurnId) {
        try { await steerSubmittedPrompt(target, clientRequestId, current.thread.activeTurnId, device); }
        catch (caught) {
          // Persisted acceptance must clear this draft even if delivery needs retry.
          if (isCurrent()) setError(t('workbench.messageSavedButSteerCouldNotBe', { value1: caught instanceof Error ? caught.message : String(caught) }));
        }
      }
    });
  }
  const loadEarlier = useCallback(async () => {
    const current = detailRef.current;
    if (!threadId || !current?.turns.length || owner.earlier) return;
    owner.earlier = true; setLoadingEarlier(true);
    try {
      const next = await fetchThreadDetail(threadId, { limit: 3, beforeTurnId: current.turns[0]!.id }, device);
      if (isCurrent()) setDetail(value => value ? { ...value, turns: prependTurns(value.turns, next.turns) } : value);
    } catch (caught) {
      if (isCurrent()) setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      owner.earlier = false;
      if (isCurrent()) setLoadingEarlier(false);
    }
  }, [threadId, owner, setDetail, setError, setLoadingEarlier, device]);
  return {
    unavailable,
    deviceId: device, detail, error, busy, canControl, access, accessReady, disabledReason, capabilities, models,
    onEvent, loadEarlier, loadingEarlier, respondingRequestId, send,
    interrupt: () => mutate(() => interruptThread(threadId!, {}, device)),
    cancelQueued: (queueId: string) => mutate(() => cancelPendingSteer(threadId!, queueId, device)),
    steerQueued: (queueId: string) => mutate(() => steerPendingPrompt(threadId!, queueId, device)),
    updateSettings: (input: UpdateThreadSettingsInput) => mutate(() => updateThreadSettings(threadId!, input, device)),
    respond: async (requestId: string, input: { answers: Record<string, { answers: string[] }> }) => {
      setRespondingRequestId(requestId);
      try { await mutate(() => respondToThreadRequest(threadId!, requestId, input, device)); }
      finally { setRespondingRequestId(null); }
    },
    retry: () => { setError(null); setRevision(r => r + 1); },
  };
}
