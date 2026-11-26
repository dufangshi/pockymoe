import { useCallback, useEffect, useState } from 'react';
import type { ModelOptionDto, ThreadTurnDto } from '@pockymoe/shared';
import { deviceRequest } from '../lib/api';

/** Read each historical harness's catalog on the thread's device, just as the model picker does. */
export function useHandoffModelNames(turns: ThreadTurnDto[] | undefined, deviceId?: string | null) {
  const harnesses = new Map<string, string>();
  for (const turn of turns ?? []) {
    for (const item of turn.items) {
      if (!item.harnessSwitch) continue;
      for (const agent of [item.harnessSwitch.fromAgentId, item.harnessSwitch.agentId]) {
        if (agent) harnesses.set(agent, turn.provider || 'acp');
      }
    }
  }
  const key = JSON.stringify([...harnesses].sort());
  const [catalog, setCatalog] = useState<{ deviceId: string | null | undefined; key: string; names: Map<string, string> }>();
  useEffect(() => {
    let active = true;
    const entries = JSON.parse(key) as [string, string][];
    void Promise.all(entries.map(async ([agent, provider]) => {
      try {
        const models = await deviceRequest<ModelOptionDto[]>(deviceId, `api/agent-runtimes/${encodeURIComponent(provider)}/models?agentId=${encodeURIComponent(agent)}`);
        return models.flatMap(model => [model.id, model.model].map(id => [`${agent}\0${id}`, model.displayName] as const));
      } catch { return []; } // Offline/removed harnesses keep their recorded model name.
    })).then(results => { if (active) setCatalog({ deviceId, key, names: new Map(results.flat()) }); });
    return () => { active = false; };
  }, [deviceId, key]);
  return useCallback((agentId: string, model: string) => catalog && catalog.deviceId === deviceId && catalog.key === key ? catalog.names.get(`${agentId}\0${model}`) : undefined, [catalog, deviceId, key]);
}
