/** The device Supervisor is the sole owner of definitions and execution state. */
export type AutomationTrigger =
  | { kind: 'interval'; everySeconds: number; anchorAt?: string | null }
  | { kind: 'at'; at: string }
  | { kind: 'threadEnded'; sourceThreadId: string }
  | { kind: 'turnEnded'; sourceThreadId: string; turnId: string }
  | { kind: 'taskEnded'; rootThreadId: string; taskNumber: number }
  | {
      kind: 'commandEnded';
      sourceThreadId: string;
      commandId?: string | null;
      commandKey?: string | null;
    };
export type AutomationCondition =
  | { kind: 'all' | 'any'; conditions: AutomationCondition[] }
  | { kind: 'not'; condition: AutomationCondition }
  | { kind: 'statusIn'; values: string[] }
  | { kind: 'exitCodeEquals'; value: number }
  | { kind: 'workspaceId' | 'commandId'; value: string };
export interface AutomationCommandSpec {
  argv?: string[];
  shell?: string | null;
  cwd: string;
  timeoutSeconds?: number;
  env?: Record<string, string>;
}
export type AutomationAction =
  | { kind: 'pluginAction'; pluginId: string; action: string; input?: unknown; timeoutSeconds?: number }
  | { kind: 'prompt'; text: string }
  | {
      kind: 'notifyInbox';
      subject: string;
      text: string;
      messageKind?: 'result' | 'status';
      includeClosingMessage?: boolean;
    }
  | ({ kind: 'runScript' } & AutomationCommandSpec);
export interface AutomationDefinition {
  name: string;
  trigger: AutomationTrigger;
  condition?: AutomationCondition;
  action: AutomationAction;
  enabled?: boolean;
  maxLatenessSeconds?: number;
  missedRunPolicy?: 'coalesceLatest' | 'skip';
  replayExisting?: boolean;
}
export interface AutomationDto {
  id: string;
  threadId: string;
  sourceKind: 'supervisor';
  definition: AutomationDefinition;
  state: 'enabled' | 'paused' | 'cancelled';
  nextRunAt: string | null;
  createdAt: string;
  updatedAt: string;
  /** Absent on older Supervisors. */
  statistics?: AutomationStatisticsDto;
  pendingCount: number;
  missedCount: number;
  error: string | null;
}
export interface AutomationRunDto {
  id: string;
  automationId: string;
  occurrenceKey: string;
  state: string;
  scheduledAt: string;
  observedAt: string;
  missedCount: number;
  pendingSteerId: string | null;
  turnId: string | null;
  commandId: string | null;
  startedAt: string | null;
  completedAt: string | null;
  deliveryReceipt: Record<string, unknown> | null;
  error: string | null;
  attemptCount: number;
}

/** Lifetime observed occurrences, not just the latest execution-history page. */
export interface AutomationStatisticsDto {
  /** Includes merged ticks and condition/loop/lateness-skipped occurrences. */
  triggerCount: number;
  runCount: number;
  executedActionCount: number;
  runningActionCount: number;
  /** Distinct bound prompt turns; tokens/prices exclude ambiguous ownership. */
  promptTurnCount: number;
  ambiguousTurnCount: number;
  missingTurnCount: number;
  unattributedRunCount: number;
  usageTurnCount: number;
  pricedTurnCount: number;
  tokenUsage: import('./index').ThreadTurnTokenBreakdownDto | null;
  priceEstimate: Pick<
    import('./index').ThreadTurnPriceEstimateDto,
    | 'totalUsd'
    | 'inputUsd'
    | 'cachedInputUsd'
    | 'cacheWriteInputUsd'
    | 'outputUsd'
  > | null;
}
