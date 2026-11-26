import type { ModelOptionDto, ThreadDto } from '@pockymoe/shared';
import { agentDisplayName, threadAgentKey } from '@pockymoe/thread-ui';

/** The name a thread's agent goes by in the composer and top bar: the device's own
    name for it (its agent list), else its family name; an ACP Codex or Claude thread
    reads "ACP · Codex" (agentDisplayName). Null for a thread without an agent. */
export function composerAgentLabel(
  thread: Pick<ThreadDto, 'provider' | 'agentId' | 'harnessProtocol'>,
  agentOptions: ModelOptionDto[] = [],
): string | null {
  const key = threadAgentKey(thread);
  if (!key) return null;
  const entry = agentOptions.find((option) => option.model === key)
    ?? agentOptions.find((option) => option.model === thread.agentId);
  return agentDisplayName(key, entry?.displayName);
}
