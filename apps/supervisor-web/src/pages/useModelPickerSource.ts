import { useEffect, useMemo, useRef, useState } from 'react';
import type { ModelOptionDto, ThreadDto } from '@pockymoe/shared';
import { modelFactsFromCatalog, type ComposerModelPickerSource, type HandoffSummary } from '@pockymoe/thread-ui';
import { deviceRequest, interruptThread } from '../lib/api';
import { fetchThreadEnvironment } from '../lib/environmentApi';
import type { ModelPricingCatalog } from '../lib/modelPricingApi';

/** How long a listed agent's models are reused before the picker asks again. */
const MODELS_FRESH_MS = 60_000;

/** The default model of an agent, else its first visible one. */
const defaultModel = (models: ModelOptionDto[]) =>
  models.find((model) => model.isDefault && !model.hidden) ?? models.find((model) => !model.hidden) ?? models[0];

/**
 * What the composer's model picker needs from this device: model facts from
 * its pricing catalog and, for the thread's owner, moving the thread to another
 * agent (docs/harness-switching.md). Switching and forking are owner-only
 * endpoints; a share recipient gets the current agent's models only.
 */
export function useModelPickerSource({ thread, deviceId, cwd, agentOptions, canSwitch, onSwitched, onForked }: {
  thread: ThreadDto | null | undefined;
  deviceId: string | null | undefined;
  cwd: string | null | undefined;
  /** The device's ACP agents (`api/agent-runtimes/acp/agents`) as the page loaded them;
      left out, the hook reads them itself (a split pane on another device). */
  agentOptions?: ModelOptionDto[] | undefined;
  canSwitch: boolean;
  onSwitched: (thread: ThreadDto) => void;
  /** Left out where the pane does not fork (the split pane). */
  onForked?: ((thread: ThreadDto) => void) | undefined;
}): ComposerModelPickerSource | undefined {
  const [pricing, setPricing] = useState<{ deviceId: string | null | undefined; catalog: ModelPricingCatalog | null }>();
  // The catalog is the device's own; read it once per device, quietly.
  useEffect(() => {
    if (!canSwitch) return;
    let active = true;
    deviceRequest<ModelPricingCatalog>(deviceId, 'api/config/model-pricing', { cache: 'no-store' })
      .then((catalog) => { if (active) setPricing({ deviceId, catalog }); })
      .catch(() => { if (active) setPricing({ deviceId, catalog: null }); });
    return () => { active = false; };
  }, [canSwitch, deviceId]);
  const catalog = pricing && pricing.deviceId === deviceId ? pricing.catalog : null;

  // A pane without the page's agent list reads it, but only for a thread on the
  // host: an isolated thread's environment cannot run the host's agents.
  const [ownAgents, setOwnAgents] = useState<{ key: string; agents: ModelOptionDto[] }>();
  const ownKey = `${deviceId ?? 'local'}\u0000${thread?.id ?? ''}`;
  const readOwn = canSwitch && agentOptions === undefined && Boolean(thread?.id) && thread?.provider === 'acp';
  useEffect(() => {
    if (!readOwn || !thread?.id) return;
    let active = true;
    const threadId = thread.id;
    void fetchThreadEnvironment(threadId, deviceId)
      .then((binding) => binding.environment === null
        ? deviceRequest<ModelOptionDto[]>(deviceId, 'api/agent-runtimes/acp/agents', { cache: 'no-store' })
        : [])
      .catch(() => [])
      .then((agents) => { if (active) setOwnAgents({ key: ownKey, agents }); });
    return () => { active = false; };
  }, [readOwn, ownKey, thread?.id, deviceId]);
  const knownAgents = agentOptions ?? (ownAgents?.key === ownKey ? ownAgents.agents : []);

  const models = useRef(new Map<string, { at: number; request: Promise<ModelOptionDto[]> }>());
  const callbacks = useRef({ onSwitched, onForked });
  callbacks.current = { onSwitched, onForked };
  const canFork = Boolean(onForked);

  const threadId = thread?.id;
  const currentAgentId = thread?.provider === 'acp' ? thread.agentId ?? null : null;
  const agentKey = knownAgents
    .filter((entry) => entry.acpAgent?.availability === 'ready' || entry.model === currentAgentId)
    .map((entry) => `${entry.model}\u0000${entry.displayName}`)
    .join('\u0001');

  return useMemo<ComposerModelPickerSource | undefined>(() => {
    const facts = catalog ? (model: ModelOptionDto) => modelFactsFromCatalog(catalog, model) : undefined;
    const agents = agentKey ? agentKey.split('\u0001').map((entry) => {
      const [id, name] = entry.split('\u0000') as [string, string];
      return { id, name: name || id };
    }) : [];
    if (!canSwitch || !threadId || !currentAgentId || agents.length < 2) return facts ? { facts } : undefined;
    const base = `api/threads/${encodeURIComponent(threadId)}/harness`;
    const loadModels = (agentId: string) => {
      const key = `${deviceId ?? 'local'}\u0000${agentId}`;
      const cached = models.current.get(key);
      if (cached && Date.now() - cached.at < MODELS_FRESH_MS) return cached.request;
      const query = new URLSearchParams({ agentId, ...(cwd ? { cwd } : {}) });
      const request = deviceRequest<ModelOptionDto[]>(deviceId, `api/agent-runtimes/acp/models?${query}`, { cache: 'no-store' });
      models.current.set(key, { at: Date.now(), request });
      request.catch(() => models.current.delete(key));
      return request;
    };
    const target = (agentId: string, model: string | null, reasoningEffort: string | null = null) =>
      ({ provider: 'acp', agentId, model, reasoningEffort });
    return {
      ...(facts ? { facts } : {}),
      switching: {
        currentAgentId,
        agents,
        loadModels,
        preview: ({ agentId, model }) => deviceRequest<HandoffSummary>(deviceId, `${base}/preview`, {
          method: 'POST',
          body: JSON.stringify(target(agentId, model)),
        }),
        switchTo: async ({ agentId, model, reasoningEffort, stop }) => {
          if (stop) {
            await interruptThread(threadId, {}, deviceId);
            // Release waits for the interrupted turn to settle before the harness changes.
            await deviceRequest(deviceId, `api/threads/${encodeURIComponent(threadId)}/release`, { method: 'POST', body: JSON.stringify({ interrupt: true }) });
          }
          const switched = await deviceRequest<ThreadDto>(deviceId, `${base}/switch`, {
            method: 'POST',
            body: JSON.stringify(target(agentId, model, reasoningEffort)),
          });
          callbacks.current.onSwitched(switched);
        },
        ...(canFork ? { fork: async ({ agentId, turnId }: { agentId: string; turnId: string | null }) => {
          // A fork starts on the agent's default model; the new thread's picker changes it.
          const model = defaultModel(await loadModels(agentId).catch(() => []))?.model ?? null;
          const forked = await deviceRequest<ThreadDto>(deviceId, `${base}/fork`, {
            method: 'POST',
            body: JSON.stringify({ target: target(agentId, model), turnId }),
          });
          callbacks.current.onForked?.(forked);
        } } : {}),
      },
    };
  }, [agentKey, canSwitch, canFork, catalog, currentAgentId, cwd, deviceId, threadId]);
}
