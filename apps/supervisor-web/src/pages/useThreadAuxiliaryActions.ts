import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  useCallback,
  useEffect,
  useState,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from 'react';
import type { NavigateFunction } from 'react-router-dom';

import type {
  ThreadDetailDto,
  ThreadDto,
  ThreadExportTurnOptionsDto,
  ThreadForkTurnOptionDto,
  ThreadHooksDto,
  ThreadMcpServersDto,
  ThreadSkillsDto,
} from '../../../../packages/shared/src/index';
import {
  ApiError,
  clearThreadGoal,
  createThreadHook,
  fetchThreadExportTurns,
  fetchThreadForkTurns,
  fetchThreadGoal,
  fetchThreadHooks,
  fetchThreadMcpServers,
  fetchThreadSkills,
  forkThread,
  trustThreadHook,
  untrustThreadHook,
  updateThreadGoal,
  updateThreadHook,
} from '../lib/api';
import { loadExportSnapshot, renderStandaloneTranscript } from '../lib/transcriptExport';
import { currentThreadHref } from '../lib/relayRoutes';
import { mergeGoalHistory, mergeThreadIntoList } from './threadDetailModel';

export interface SlashPanelState<T> {
  status: 'idle' | 'loading' | 'ready' | 'failed';
  data: T | null;
  error: string | null;
}

interface UseThreadAuxiliaryActionsInput {
  detailRef: MutableRefObject<ThreadDetailDto | null>;
  id: string;
  navigate: NavigateFunction;
  setDetail: Dispatch<SetStateAction<ThreadDetailDto | null>>;
  setError: Dispatch<SetStateAction<string | null>>;
  setThreads: Dispatch<SetStateAction<ThreadDto[]>>;
}

const idlePanelState = <T,>(): SlashPanelState<T> => ({
  status: 'idle',
  data: null,
  error: null,
});

export function useThreadAuxiliaryActions({
  detailRef,
  id,
  navigate,
  setDetail,
  setError,
  setThreads,
}: UseThreadAuxiliaryActionsInput) {
  useI18n();
  const [skillsState, setSkillsState] = useState<SlashPanelState<ThreadSkillsDto>>(
    idlePanelState,
  );
  const [mcpState, setMcpState] = useState<SlashPanelState<ThreadMcpServersDto>>(
    idlePanelState,
  );
  const [hooksState, setHooksState] = useState<SlashPanelState<ThreadHooksDto>>(
    idlePanelState,
  );
  const [forkTurnOptionsState, setForkTurnOptionsState] = useState<
    SlashPanelState<ThreadForkTurnOptionDto[]>
  >(idlePanelState);
  const [goalState, setGoalState] = useState<
    SlashPanelState<ThreadDetailDto['goal']>
  >(idlePanelState);
  const [goalMonitorOpen, setGoalMonitorOpen] = useState(false);
  const [goalActionBusy, setGoalActionBusy] = useState(false);
  const [expandedGoalIds, setExpandedGoalIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [exportDialogOpen, setExportDialogOpen] = useState(false);
  const [exportBusy, setExportBusy] = useState(false);
  const [exportTurnsState, setExportTurnsState] = useState<
    SlashPanelState<ThreadExportTurnOptionsDto>
  >(idlePanelState);

  useEffect(() => {
    setSkillsState(idlePanelState);
    setMcpState(idlePanelState);
    setHooksState(idlePanelState);
    setForkTurnOptionsState(idlePanelState);
    setGoalState(idlePanelState);
    setGoalMonitorOpen(false);
    setExpandedGoalIds(new Set());
    setExportDialogOpen(false);
    setExportTurnsState(idlePanelState);
  }, [id]);

  const loadExportTurns = useCallback(async () => {
    if (!id) {
      return;
    }

    setExportTurnsState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await fetchThreadExportTurns(id);
      setExportTurnsState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      const message =
        requestError instanceof ApiError
          ? requestError.payload.message
          : translate("workbench.unableToLoadExportTurns");
      setExportTurnsState((current) => ({
        status: 'failed',
        data: current.data,
        error: message,
      }));
    }
  }, [id]);

  async function handleExportTranscript(
    input: import('@pockymoe/shared').ExportThreadTranscriptInput,
  ) {
    if (!id) {
      return;
    }

    setError(null);
    setExportBusy(true);

    try {
      const snapshot = await loadExportSnapshot(id, input);
      const html = await renderStandaloneTranscript(snapshot);
      const filename = `pockymoe-${snapshot.title.replace(/[^a-z0-9._-]+/gi, '-').replace(/^-|-$/g, '') || 'thread'}.html`;
      if (window.pockymoeHost && await (await import('../desktop/download')).nativeSave(new Blob([html], { type: 'text/html;charset=utf-8' }), filename)) {
        setExportDialogOpen(false); return;
      }
      const href = URL.createObjectURL(
        new Blob([html], { type: 'text/html;charset=utf-8' }),
      );
      const anchor = document.createElement('a');
      anchor.href = href;
      anchor.download = filename;
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      window.setTimeout(() => URL.revokeObjectURL(href), 30_000);
      setExportDialogOpen(false);
    } catch (requestError) {
      const message =
        requestError instanceof ApiError
          ? requestError.payload.message
          : requestError instanceof Error
            ? requestError.message
            : translate("workbench.unableToExportTranscript");
      setError(message);
    } finally {
      setExportBusy(false);
    }
  }

  async function handleOpenGoal() {
    if (!id) {
      return;
    }

    setGoalState((current) => ({
      status: 'loading',
      data: current.data ?? detailRef.current?.goal ?? null,
      error: null,
    }));

    try {
      const next = await fetchThreadGoal(id);
      setGoalState({
        status: 'ready',
        data: next.goal,
        error: null,
      });
      setDetail((current) =>
        current
          ? next.goal
            ? {
                ...current,
                goal: next.goal,
                goalHistory: mergeGoalHistory(current.goalHistory ?? [], next.goal),
              }
            : {
                ...current,
                goal: next.goal,
              }
          : current,
      );
    } catch (requestError) {
      setGoalState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToLoadGoal"),
      }));
    }
  }

  async function handleUpdateGoal(input: {
    objective?: string | null;
    status?: NonNullable<ThreadDetailDto['goal']>['status'] | null;
    tokenBudget?: number | null;
  }) {
    if (!id) {
      return;
    }

    setGoalState((current) => ({
      status: 'loading',
      data: current.data ?? detailRef.current?.goal ?? null,
      error: null,
    }));

    try {
      const next = await updateThreadGoal(id, input);
      setGoalState({
        status: 'ready',
        data: next.goal,
        error: null,
      });
      setDetail((current) =>
        current
          ? next.goal
            ? {
                ...current,
                goal: next.goal,
                goalHistory: mergeGoalHistory(current.goalHistory ?? [], next.goal),
              }
            : {
                ...current,
                goal: next.goal,
              }
          : current,
      );
    } catch (requestError) {
      setGoalState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToUpdateGoal"),
      }));
      throw requestError;
    }
  }

  async function handleClearGoal() {
    if (!id) {
      return;
    }

    setGoalState((current) => ({
      status: 'loading',
      data: current.data ?? detailRef.current?.goal ?? null,
      error: null,
    }));

    try {
      const next = await clearThreadGoal(id);
      setGoalState({
        status: 'ready',
        data: null,
        error: null,
      });
      setDetail((current) =>
        current
          ? next.goalHistory
            ? {
                ...current,
                goal: null,
                goalHistory: next.goalHistory,
              }
            : {
                ...current,
                goal: null,
              }
          : current,
      );
    } catch (requestError) {
      setGoalState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToClearGoal"),
      }));
      throw requestError;
    }
  }

  async function handleGoalStatusAction(
    status: NonNullable<ThreadDetailDto['goal']>['status'],
  ) {
    setGoalActionBusy(true);
    try {
      await handleUpdateGoal({ status });
    } finally {
      setGoalActionBusy(false);
    }
  }

  async function handleTerminateGoal() {
    setGoalActionBusy(true);
    try {
      await handleClearGoal();
    } finally {
      setGoalActionBusy(false);
    }
  }

  async function handleOpenSkills() {
    if (!id) {
      return;
    }

    setSkillsState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await fetchThreadSkills(id);
      setSkillsState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setSkillsState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToLoadSkills"),
      }));
    }
  }

  async function handleOpenMcp() {
    if (!id) {
      return;
    }

    setMcpState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await fetchThreadMcpServers(id);
      setMcpState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setMcpState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToLoadMCPServers"),
      }));
    }
  }

  async function handleOpenHooks() {
    if (!id) {
      return;
    }

    setHooksState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await fetchThreadHooks(id);
      setHooksState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setHooksState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToLoadHooks"),
      }));
    }
  }

  async function handleCreateHook(input: Parameters<typeof createThreadHook>[1]) {
    if (!id) {
      return;
    }

    setHooksState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await createThreadHook(id, input);
      setHooksState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setHooksState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToCreateHook"),
      }));
      throw requestError;
    }
  }

  async function handleUpdateHook(input: Parameters<typeof updateThreadHook>[1]) {
    if (!id) {
      return;
    }

    setHooksState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await updateThreadHook(id, input);
      setHooksState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setHooksState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToUpdateHook"),
      }));
      throw requestError;
    }
  }

  async function handleTrustHook(input: Parameters<typeof trustThreadHook>[1]) {
    if (!id) {
      return;
    }

    setHooksState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await trustThreadHook(id, input);
      setHooksState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setHooksState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToTrustHook"),
      }));
      throw requestError;
    }
  }

  async function handleUntrustHook(input: Parameters<typeof untrustThreadHook>[1]) {
    if (!id) {
      return;
    }

    setHooksState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await untrustThreadHook(id, input);
      setHooksState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setHooksState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToUntrustHook"),
      }));
      throw requestError;
    }
  }

  async function handleOpenForkTurns() {
    if (!id) {
      return;
    }

    setForkTurnOptionsState((current) => ({
      status: 'loading',
      data: current.data,
      error: null,
    }));

    try {
      const next = await fetchThreadForkTurns(id);
      setForkTurnOptionsState({
        status: 'ready',
        data: next,
        error: null,
      });
    } catch (requestError) {
      setForkTurnOptionsState((current) => ({
        status: 'failed',
        data: current.data,
        error:
          requestError instanceof ApiError
            ? requestError.payload.message
            : translate("workbench.unableToLoadTurnsForForking"),
      }));
    }
  }

  async function handleForkLatest() {
    if (!id) {
      return;
    }

    const result = await forkThread(id, { mode: 'latest' });
    setThreads((current) => mergeThreadIntoList(current, result.thread.thread));
    navigate(currentThreadHref(result.thread.thread.id));
  }

  async function handleForkTurn(turnId: string) {
    if (!id) {
      return;
    }

    const result = await forkThread(id, { mode: 'turn', turnId });
    setThreads((current) => mergeThreadIntoList(current, result.thread.thread));
    navigate(currentThreadHref(result.thread.thread.id));
  }

  return {
    expandedGoalIds,
    exportBusy,
    exportDialogOpen,
    exportTurnsState,
    forkTurnOptionsState,
    goalActionBusy,
    goalMonitorOpen,
    goalState,
    handleCreateHook,
    handleExportTranscript,
    handleForkLatest,
    handleForkTurn,
    handleGoalStatusAction,
    handleOpenForkTurns,
    handleOpenGoal,
    handleOpenHooks,
    handleOpenMcp,
    handleOpenSkills,
    handleTerminateGoal,
    handleTrustHook,
    handleUntrustHook,
    handleUpdateGoal,
    handleUpdateHook,
    hooksState,
    loadExportTurns,
    mcpState,
    setExpandedGoalIds,
    setExportDialogOpen,
    setGoalMonitorOpen,
    setGoalState,
    skillsState,
  };
}
