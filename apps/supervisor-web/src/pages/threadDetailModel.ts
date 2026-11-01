import {
  mergeLatestThreadTurns,
  mergeThreadHistoryItem,
  prependEarlierThreadTurns,
  type ModelOptionDto,
  type ThreadDetailDto,
  type ThreadDto,
  type ThreadHistoryItemDto,
  type ThreadTurnPriceEstimateDto,
  type ThreadTurnTokenUsageDto,
} from '@pockymoe/shared';

export function prependTurns(
  existing: ThreadDetailDto['turns'],
  older: ThreadDetailDto['turns'],
) {
  return prependEarlierThreadTurns(existing, older);
}

export function appendLatestTurns(
  existing: ThreadDetailDto['turns'],
  latest: ThreadDetailDto['turns'],
  activeTurnId: string | null = null,
) {
  const existingById = new Map(existing.map((turn) => [turn.id, turn]));
  const withLiveUsage = latest.map((turn) => {
    const previous = existingById.get(turn.id);
    // A detail request may have started before the latest usage event arrived.
    // Keep the newer running total, but always accept an authoritative final turn.
    if (
      turn.status === 'inProgress' &&
      previous?.status === 'inProgress' &&
      previous.tokenUsage &&
      previous.tokenUsage.total.totalTokens > (turn.tokenUsage?.total.totalTokens ?? -1)
    ) {
      return {
        ...turn,
        model: previous.model ?? turn.model ?? null,
        reasoningEffort: previous.reasoningEffort ?? turn.reasoningEffort ?? null,
        reasoningEffortAvailable: previous.reasoningEffortAvailable ?? turn.reasoningEffortAvailable ?? null,
        tokenUsage: previous.tokenUsage,
        priceEstimate: previous.priceEstimate ?? null,
      };
    }
    return turn;
  });
  return mergeLatestThreadTurns(existing, withLiveUsage, activeTurnId);
}

export function applyLiveItemTimestampsToTurns(
  turns: ThreadDetailDto['turns'],
  liveItems: NonNullable<ThreadDetailDto['liveItems']> | null,
) {
  if (!liveItems || liveItems.items.length === 0) {
    return turns;
  }

  return turns.map((turn) => {
    if (turn.id !== liveItems.turnId) {
      return turn;
    }

    const timestampedLiveItems = liveItems.items.filter(
      (item) => item.createdAt,
    );
    if (timestampedLiveItems.length === 0) {
      return turn;
    }

    const liveItemsById = new Map(
      timestampedLiveItems.map((item) => [item.id, item]),
    );
    const usedLiveAgentIds = new Set<string>();
    let changed = false;
    const nextItems = turn.items.map((item) => {
      if (item.createdAt && item.createdAt !== turn.startedAt) {
        return item;
      }

      let liveItem = liveItemsById.get(item.id);
      if (!liveItem && item.kind === 'agentMessage') {
        liveItem = timestampedLiveItems.find(
          (candidate) =>
            candidate.kind === 'agentMessage' &&
            !usedLiveAgentIds.has(candidate.id) &&
            candidate.text.trim().length >= 8 &&
            item.text.trim().includes(candidate.text.trim()),
        );
      }

      if (!liveItem?.createdAt || liveItem.createdAt === turn.startedAt) {
        return item;
      }

      usedLiveAgentIds.add(liveItem.id);
      changed = true;
      return { ...item, createdAt: liveItem.createdAt };
    });

    return changed ? { ...turn, items: nextItems } : turn;
  });
}

export function mergePendingRequestIntoDetail(
  detail: ThreadDetailDto,
  request: ThreadDetailDto['pendingRequests'][number],
): ThreadDetailDto {
  return {
    ...detail,
    pendingRequests: [
      ...detail.pendingRequests.filter((entry) => entry.id !== request.id),
      request,
    ],
  };
}

export function removePendingRequestFromDetail(
  detail: ThreadDetailDto,
  requestId: string,
): ThreadDetailDto {
  const pendingRequests = detail.pendingRequests.filter(
    (entry) => entry.id !== requestId,
  );
  return pendingRequests.length === detail.pendingRequests.length
    ? detail
    : {
        ...detail,
        pendingRequests,
      };
}

export function mergePendingRequests(
  current: ThreadDetailDto['pendingRequests'],
  incoming: ThreadDetailDto['pendingRequests'],
  resolvedRequestIds: ReadonlySet<string>,
) {
  const filteredIncoming = incoming.filter(
    (request) => !resolvedRequestIds.has(request.id),
  );
  const filteredCurrent = current.filter(
    (request) => !resolvedRequestIds.has(request.id),
  );

  if (filteredCurrent.length === 0) {
    return filteredIncoming;
  }

  const incomingById = new Map(
    filteredIncoming.map((request) => [request.id, request] as const),
  );
  const merged = [
    ...filteredCurrent.map(
      (request) => incomingById.get(request.id) ?? request,
    ),
    ...filteredIncoming.filter(
      (request) => !filteredCurrent.some((entry) => entry.id === request.id),
    ),
  ];

  return merged.sort((left, right) =>
    left.createdAt.localeCompare(right.createdAt),
  );
}

export function isThreadActionRequest(
  value: unknown,
): value is ThreadDetailDto['pendingRequests'][number] {
  if (!value || typeof value !== 'object') {
    return false;
  }
  const request = value as Partial<ThreadDetailDto['pendingRequests'][number]>;
  return (
    typeof request.id === 'string' &&
    (request.kind === 'requestUserInput' ||
      request.kind === 'permissionRequest' ||
      request.kind === 'planDecision') &&
    typeof request.title === 'string' &&
    typeof request.createdAt === 'string' &&
    Array.isArray(request.questions)
  );
}

export function mergeLiveHistoryItem(
  current: ThreadHistoryItemDto | undefined,
  incoming: ThreadHistoryItemDto,
): ThreadHistoryItemDto {
  if (!current || current.kind !== incoming.kind) {
    return incoming;
  }

  const mergeOrderingHints = (
    item: ThreadHistoryItemDto,
  ): ThreadHistoryItemDto => {
    let nextItem = item;
    const sequence =
      typeof incoming.sequence === 'number' &&
      Number.isFinite(incoming.sequence)
        ? incoming.sequence
        : typeof current.sequence === 'number' &&
            Number.isFinite(current.sequence)
          ? current.sequence
          : null;
    if (sequence !== null && nextItem.sequence !== sequence) {
      nextItem = { ...nextItem, sequence };
    }

    const transcriptOrder =
      typeof incoming.transcriptOrder === 'number' &&
      Number.isFinite(incoming.transcriptOrder)
        ? incoming.transcriptOrder
        : typeof current.transcriptOrder === 'number' &&
            Number.isFinite(current.transcriptOrder)
          ? current.transcriptOrder
          : null;
    if (
      transcriptOrder !== null &&
      nextItem.transcriptOrder !== transcriptOrder
    ) {
      nextItem = { ...nextItem, transcriptOrder };
    }

    return nextItem;
  };

  return mergeOrderingHints(mergeThreadHistoryItem(current, incoming));
}

export function reconcileLiveItemsWithDetail(
  current: NonNullable<ThreadDetailDto['liveItems']> | null,
  incoming: NonNullable<ThreadDetailDto['liveItems']> | null,
  turns: ThreadDetailDto['turns'],
): NonNullable<ThreadDetailDto['liveItems']> | null {
  if (!current) {
    return incoming;
  }

  if (incoming && incoming.turnId !== current.turnId) {
    return incoming;
  }

  const materializedTurn = turns.find((turn) => turn.id === current.turnId);
  const materializedItemsById = new Map(
    materializedTurn?.items.map((item) => [item.id, item]) ?? [],
  );
  const materializedAgentTexts =
    materializedTurn?.items
      .filter((item) => item.kind === 'agentMessage')
      .map((item) => item.text.trim())
      .filter(Boolean) ?? [];
  const isCoveredByMaterializedAgentText = (item: ThreadHistoryItemDto) =>
    item.kind === 'agentMessage' &&
    item.text.trim().length > 0 &&
    materializedAgentTexts.some((text) => text.includes(item.text.trim()));

  const isCoveredByMaterializedItem = (item: ThreadHistoryItemDto) => {
    const materialized = materializedItemsById.get(item.id);
    if (!materialized) {
      return isCoveredByMaterializedAgentText(item);
    }
    if (
      materialized.kind !== item.kind ||
      (typeof item.sequence === 'number' &&
        Number.isFinite(item.sequence) &&
        materialized.sequence !== item.sequence)
    ) {
      return false;
    }
    if (mergeThreadHistoryItem(item, materialized).status !== materialized.status) {
      return false;
    }
    // A response fetched before the latest delta can contain the same item ID
    // while still missing its newest text. Keep that live overlay until caught up.
    if (item.kind === 'agentMessage' || item.kind === 'reasoning') {
      return (
        materialized.text.includes(item.text.trim()) &&
        (!item.detailText?.trim() ||
          (materialized.detailText ?? materialized.text).includes(item.detailText.trim()))
      );
    }
    return true;
  };

  if (!incoming) {
    const remainingItems = current.items.filter(
      (item) => !isCoveredByMaterializedItem(item),
    );
    return remainingItems.length === 0
      ? null
      : {
          ...current,
          items: remainingItems,
        };
  }

  const currentItemsById = new Map(
    current.items.map((item) => [item.id, item]),
  );
  const incomingItemsById = new Map(
    incoming.items.map((item) => [item.id, item]),
  );
  const orderedIds = [
    ...current.items.map((item) => item.id),
    ...incoming.items
      .map((item) => item.id)
      .filter((id) => !currentItemsById.has(id)),
  ];
  const items = orderedIds
    .map((id) => {
      const incomingItem = incomingItemsById.get(id);
      const currentItem = currentItemsById.get(id);
      if (!incomingItem) {
        return currentItem && !isCoveredByMaterializedItem(currentItem)
          ? currentItem
          : null;
      }
      return mergeLiveHistoryItem(currentItem, incomingItem);
    })
    .filter((item): item is ThreadHistoryItemDto => Boolean(item));

  return items.length === 0
    ? null
    : {
        turnId: incoming.turnId,
        items,
        updatedAt:
          incoming.updatedAt.localeCompare(current.updatedAt) >= 0
            ? incoming.updatedAt
            : current.updatedAt,
      };
}

export function appendLiveAgentDeltaToItems(
  current: NonNullable<ThreadDetailDto['liveItems']> | null,
  turns: ThreadDetailDto['turns'],
  event: {
    turnId: string;
    itemId: string;
    delta: string;
    text?: string | null;
    sequence: number | null;
    createdAt?: string | null;
  },
): NonNullable<ThreadDetailDto['liveItems']> {
  const { turnId, itemId, delta, sequence, createdAt } = event;
  const currentItems = current?.turnId === turnId ? current.items : [];
  const currentItem = currentItems.find((item) => item.id === itemId);
  const persisted = turns.find((turn) => turn.id === turnId)?.items.find(
    (item) => item.id === itemId && item.kind === 'agentMessage',
  );
  const existing = persisted
    ? mergeLiveHistoryItem(currentItem, persisted)
    : currentItem;
  const updatedAt = new Date().toISOString();
  const text = typeof event.text === 'string'
    ? existing?.kind === 'agentMessage' && existing.text.length > event.text.length
      ? existing.text
      : event.text
    : `${existing?.kind === 'agentMessage' ? existing.text : ''}${delta}`;
  const nextItem: ThreadHistoryItemDto = existing?.kind === 'agentMessage'
    ? {
        ...existing,
        text,
        sequence: sequence ?? existing.sequence ?? null,
      }
    : {
        id: itemId,
        kind: 'agentMessage',
        text,
        sequence,
        createdAt: createdAt ?? updatedAt,
      };
  return {
    turnId,
    items: currentItem
      ? currentItems.map((item) => item.id === itemId ? nextItem : item)
      : [...currentItems, nextItem],
    updatedAt,
  };
}

export function mergeThreadIntoList(existing: ThreadDto[], thread: ThreadDto) {
  const remaining = existing.filter((entry) => entry.id !== thread.id);
  return [thread, ...remaining];
}

function goalKey(goal: NonNullable<ThreadDetailDto['goal']>) {
  return `${goal.threadId}:${goal.objective}:${goal.createdAt}`;
}

function mergeGoalEntry(
  existing: NonNullable<ThreadDetailDto['goal']>,
  incoming: NonNullable<ThreadDetailDto['goal']>,
) {
  const existingUpdatedAt = Date.parse(existing.updatedAt) || 0;
  const incomingUpdatedAt = Date.parse(incoming.updatedAt) || 0;
  const latest = incomingUpdatedAt >= existingUpdatedAt ? incoming : existing;
  const fallback = latest === incoming ? existing : incoming;
  return {
    ...latest,
    localGoalId: latest.localGoalId ?? fallback.localGoalId ?? null,
  };
}

export function normalizeGoalHistory(
  goals: NonNullable<ThreadDetailDto['goalHistory']>,
) {
  const byKey = new Map<string, NonNullable<ThreadDetailDto['goal']>>();
  for (const goal of goals) {
    const key = goalKey(goal);
    const existing = byKey.get(key);
    byKey.set(key, existing ? mergeGoalEntry(existing, goal) : goal);
  }
  return [...byKey.values()].sort(
    (left, right) => Date.parse(right.updatedAt) - Date.parse(left.updatedAt),
  );
}

export function mergeGoalHistory(
  existing: NonNullable<ThreadDetailDto['goalHistory']>,
  goal: NonNullable<ThreadDetailDto['goal']>,
) {
  return normalizeGoalHistory([goal, ...existing]);
}

export function getReasoningEffortAvailability(
  modelOptions: ModelOptionDto[],
  model: string | null,
) {
  if (!model) {
    return null;
  }

  const matchedModel = modelOptions.find((entry) => entry.model === model);
  if (!matchedModel) {
    return null;
  }

  return matchedModel.supportedReasoningEfforts.length > 1;
}

export function createClientRequestId() {
  if (
    typeof crypto !== 'undefined' &&
    typeof crypto.randomUUID === 'function'
  ) {
    return crypto.randomUUID();
  }

  return `client-${Date.now()}-${Math.random().toString(16).slice(2, 10)}`;
}

export function turnHasUserMessage(
  turn: ThreadDetailDto['turns'][number] | undefined,
  prompt: string,
) {
  return (
    turn?.items.some(
      (item) =>
        item.kind === 'userMessage' && item.text.trim() === prompt.trim(),
    ) ?? false
  );
}

const PHOTO_PLACEHOLDER_PATTERN = /\s*\[(?:PHOTO\s+[^\]]+|localImage)\]\s*/g;

export function promptHasPhotoPlaceholder(prompt: string) {
  return /\[(?:PHOTO\s+[^\]]+|localImage)\]/.test(prompt);
}

function promptWithoutPhotoTokens(prompt: string) {
  return prompt
    .replace(PHOTO_PLACEHOLDER_PATTERN, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

export function turnHasPhotoPromptText(
  turn: ThreadDetailDto['turns'][number] | undefined,
  prompt: string,
) {
  const normalizedPrompt = promptWithoutPhotoTokens(prompt);
  if (!normalizedPrompt) {
    return false;
  }

  return (
    turn?.items.some(
      (item) =>
        item.kind === 'userMessage' &&
        promptWithoutPhotoTokens(item.text) === normalizedPrompt,
    ) ?? false
  );
}

export function turnHasPhotoAttachment(
  turn: ThreadDetailDto['turns'][number] | undefined,
) {
  return (
    turn?.items.some(
      (item) =>
        item.kind === 'userMessage' &&
        /\[(?:PHOTO\s+\.\/\.temp\/threads\/[^\]]+|localImage)\]/.test(
          item.text,
        ),
    ) ?? false
  );
}

export function findTurnWithUserMessage(
  turns: ThreadDetailDto['turns'],
  prompt: string,
) {
  return (
    turns.find((turn) => turnHasUserMessage(turn, prompt)) ??
    (promptHasPhotoPlaceholder(prompt)
      ? (turns.find((turn) => turnHasPhotoPromptText(turn, prompt)) ?? null)
      : null)
  );
}

export function findMaterializedOptimisticTurn(
  turns: ThreadDetailDto['turns'],
  optimistic: { id: string; serverTurnId: string | null; prompt: string },
) {
  return (
    turns.find((turn) => {
      const matchesAuthoritativeId =
        (optimistic.serverTurnId !== null &&
          turn.id === optimistic.serverTurnId) ||
        turn.id === optimistic.id;
      if (
        matchesAuthoritativeId &&
        turn.items.some((item) => item.kind === 'userMessage')
      ) {
        return true;
      }

      return (
        turnHasUserMessage(turn, optimistic.prompt) ||
        (promptHasPhotoPlaceholder(optimistic.prompt) &&
          (turnHasPhotoPromptText(turn, optimistic.prompt) ||
            turnHasPhotoAttachment(turn)))
      );
    }) ?? null
  );
}

export function mergeTurnTokenUsage(
  turns: ThreadDetailDto['turns'],
  turnId: string,
  tokenUsage: ThreadTurnTokenUsageDto,
  priceEstimate: ThreadTurnPriceEstimateDto | null,
  metadata: Partial<Pick<ThreadDetailDto['turns'][number], 'model' | 'reasoningEffort' | 'reasoningEffortAvailable'>> = {},
) {
  let changed = false;
  const nextTurns = turns.map((turn) => {
    if (turn.id !== turnId) {
      return turn;
    }

    changed = true;
    return {
      ...turn,
      ...metadata,
      tokenUsage,
      priceEstimate,
    };
  });

  return changed ? nextTurns : turns;
}

// A plain usage update says nothing about categories: the last analysis stays, and
// the context panel says so when the total has moved on from it.
export function mergeThreadContextUsage(
  current: ThreadDto['contextUsage'],
  next: NonNullable<ThreadDto['contextUsage']>,
): NonNullable<ThreadDto['contextUsage']> {
  return next.details || !current?.details ? next : { ...next, details: current.details };
}

// Older supervisors expose Codex's last request in turn usage but omit the
// thread context DTO. Keep those devices useful during a rolling upgrade.
export function resolveThreadContextUsage(detail: Pick<ThreadDetailDto, 'thread' | 'turns'>): ThreadDto['contextUsage'] {
  if (detail.thread.contextUsage?.availability === 'available') return detail.thread.contextUsage;
  for (const turn of [...detail.turns].reverse()) {
    const usage = turn.tokenUsage;
    const size = usage?.modelContextWindow;
    const used = usage?.last?.totalTokens;
    if (typeof size !== 'number' || !Number.isFinite(size) || size <= 0 || typeof used !== 'number' || !Number.isFinite(used) || used < 0) continue;
    return { availability:'available', tokensInContextWindow:used, modelContextWindow:size,
      remainingPercent:Math.round(100 * Math.max(0,size-used)/size),
      updatedAt:turn.completedAt ?? turn.startedAt ?? detail.thread.updatedAt };
  }
  return detail.thread.contextUsage;
}
