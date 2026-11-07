import type { AgentBackendIdDto, ModelOptionDto } from '@pockymoe/shared';
import { ApiError, fetchAgentBackendAgents, fetchAgentBackendModelsFor, fetchAgentBackendStatus, fetchThreadModels } from '../lib/api';
import { fetchEnvironments, fetchThreadEnvironment } from '../lib/environmentApi';

async function allowsHostDiscovery(threadId: string | undefined, deviceId: string | null) {
  if (!threadId) return false;
  try {
    const binding = await fetchThreadEnvironment(threadId, deviceId);
    // Only an explicit null confirms Host. A malformed response is unknown.
    return binding.environment === null;
  } catch (error) {
    if (!(error instanceof ApiError && error.statusCode === 404)) return false;
    try {
      await fetchEnvironments(deviceId);
      // The environment API exists, so this thread's missing binding is unknown.
      return false;
    } catch (capabilityError) {
      // Legacy Host compatibility requires BOTH environment endpoints to be absent.
      // Permission, network and server failures never authorize a Host fallback.
      return capabilityError instanceof ApiError && capabilityError.statusCode === 404;
    }
  }
}

/** Thread models stay scoped; Host discovery waits for the isolation boundary. */
export function requestThreadPageCatalog({ provider, agentId, cwd, threadId, deviceId, isCurrent }: {
  provider: AgentBackendIdDto;
  agentId: string | null;
  cwd: string | null;
  threadId: string | undefined;
  deviceId: string | null;
  isCurrent: () => boolean;
}) {
  const hostAllowed = allowsHostDiscovery(threadId, deviceId);
  const fallbackModels = async (): Promise<ModelOptionDto[]> => {
    if (!await hostAllowed || !isCurrent()) return [];
    return fetchAgentBackendModelsFor(provider, { ...(provider === 'acp' && agentId ? { agentId } : {}), cwd });
  };
  const modelRequest = threadId ? fetchThreadModels(threadId).catch(async error => {
    if (error instanceof ApiError && error.statusCode === 404) return fallbackModels();
    throw error;
  }) : Promise.resolve([] as ModelOptionDto[]);
  const agentRequest = provider === 'acp' ? hostAllowed.then(allowed =>
    allowed && isCurrent() ? fetchAgentBackendAgents(provider) : [] as ModelOptionDto[],
  ) : Promise.resolve([] as ModelOptionDto[]);
  const statusRequest = hostAllowed.then(allowed => allowed && isCurrent() ? fetchAgentBackendStatus(provider) : null);
  return { modelRequest, agentRequest, statusRequest };
}
