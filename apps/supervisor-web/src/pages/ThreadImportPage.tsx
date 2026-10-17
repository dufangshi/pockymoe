import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { ProductHeader } from '../components/ProductHeader';
import type { FormEvent } from 'react';
import { useEffect, useMemo, useState } from 'react';
import { ArrowLeft, RefreshCw, Search } from 'lucide-react';
import { useNavigate } from 'react-router-dom';

import type {
  AgentBackendDto,
  AgentBackendIdDto,
  ImportThreadCandidateDto,
  ModelOptionDto,
} from '@pockymoe/shared';
import { defaultAgentBackendId } from '@pockymoe/shared';
import {
  ApiError,
  fetchAgentBackends,
  fetchAgentBackendAgents,
  fetchImportThreadCandidates,
  importThread,
} from '../lib/api';
import { parseSessionRef, providerForImportedAgent } from '../lib/importSessionId';
import { currentThreadHref, currentWorkspacesHref } from '../lib/relayRoutes';

function canImportFromBackend(backend: AgentBackendDto) {
  return backend.enabled && backend.capabilities.sessions.importLocal;
}

export function ThreadImportPage() {
  useI18n();
  const navigate = useNavigate();
  const [sessionId, setSessionId] = useState('');
  const [provider, setProvider] = useState<AgentBackendIdDto>(defaultAgentBackendId);
  const [backends, setBackends] = useState<AgentBackendDto[]>([]);
  const [backendsLoading, setBackendsLoading] = useState(true);
  const [backendsError, setBackendsError] = useState<string | null>(null);
  const [backendLoadAttempt, setBackendLoadAttempt] = useState(0);
  const [agents, setAgents] = useState<ModelOptionDto[]>([]);
  const [agentId, setAgentId] = useState<string | null>(null);
  const [agentsLoading, setAgentsLoading] = useState(false);
  const [candidates, setCandidates] = useState<ImportThreadCandidateDto[]>([]);
  const [candidatesLoading, setCandidatesLoading] = useState(true);
  const [candidatesError, setCandidatesError] = useState<string | null>(null);
  const [candidateLoadAttempt, setCandidateLoadAttempt] = useState(0);
  const [candidateQuery, setCandidateQuery] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setBackendsLoading(true);
    setBackendsError(null);
    fetchAgentBackends()
      .then((loaded) => {
        if (cancelled) {
          return;
        }
        setBackends(loaded);
        const preferred =
          loaded.find((backend) => backend.isDefault && canImportFromBackend(backend))?.provider ??
          loaded.find(canImportFromBackend)?.provider ??
          loaded[0]?.provider ??
          defaultAgentBackendId;
        setProvider(preferred);
      })
      .catch((caught) => {
        if (!cancelled) {
          setBackends([]);
          setBackendsError(
            caught instanceof Error ? caught.message : translate("workbench.unableToLoadBackends"),
          );
        }
      })
      .finally(() => {
        if (!cancelled) {
          setBackendsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [backendLoadAttempt]);

  useEffect(() => {
    let cancelled = false;
    if (provider !== 'acp') {
      setAgents([]);
      setAgentId(null);
      setAgentsLoading(false);
      return () => {
        cancelled = true;
      };
    }
    setAgentsLoading(true);
    fetchAgentBackendAgents('acp')
      .then((loaded) => {
        if (cancelled) return;
        setAgents(loaded);
        setAgentId((current) => {
          if (current && loaded.some((agent) => agent.id === current)) {
            return current;
          }
          return loaded.find((agent) => agent.acpAgent?.availability === 'ready')?.id ?? null;
        });
      })
      .catch(() => {
        if (!cancelled) {
          setAgents([]);
          setAgentId(null);
        }
      })
      .finally(() => {
        if (!cancelled) setAgentsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [provider]);

  useEffect(() => {
    let cancelled = false;
    if (provider === 'acp' && !agentId) {
      setCandidates([]);
      setCandidatesLoading(agentsLoading);
      return () => {
        cancelled = true;
      };
    }
    setCandidatesLoading(true);
    setCandidatesError(null);
    fetchImportThreadCandidates(provider, agentId)
      .then((loaded) => {
        if (!cancelled) {
          setCandidates(loaded);
        }
      })
      .catch((caught) => {
        if (!cancelled) {
          setCandidates([]);
          setCandidatesError(
            caught instanceof Error ? caught.message : translate("workbench.unableToListLocalSessions"),
          );
        }
      })
      .finally(() => {
        if (!cancelled) {
          setCandidatesLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, agentsLoading, provider, candidateLoadAttempt]);

  const backendOptions = backends;
  const selectedBackend = backendOptions.find((backend) => backend.provider === provider) ?? null;
  const selectedAgent = agents.find((agent) => agent.id === agentId) ?? null;
  const canImportSelection = Boolean(
    selectedBackend &&
      canImportFromBackend(selectedBackend) &&
      (provider !== 'acp' || selectedAgent?.acpAgent?.availability === 'ready'),
  );
  const normalizedCandidateQuery = candidateQuery.trim().toLocaleLowerCase();
  const filteredCandidates = useMemo(
    () =>
      normalizedCandidateQuery
        ? candidates.filter((candidate) =>
            `${candidate.title} ${candidate.sessionId} ${candidate.cwd ?? ''} ${candidate.preview ?? ''}`
              .toLocaleLowerCase()
              .includes(normalizedCandidateQuery),
          )
        : candidates,
    [candidates, normalizedCandidateQuery],
  );
  const parsedSession = parseSessionRef(sessionId);
  const selectedCandidate = candidates.find(
    (candidate) => candidate.sessionId === parsedSession.rawId,
  ) ?? null;

  function applyPastedSession(value: string) {
    setError(null);
    const parsed = parseSessionRef(value);
    const looksComplete =
      Boolean(parsed.rawId) &&
      parsed.rawId !== value.trim() &&
      /[0-9a-f]{8}-[0-9a-f-]{4,}/i.test(parsed.rawId);
    setSessionId(looksComplete ? parsed.rawId : value);
    if (!parsed.agentId || !looksComplete) {
      return;
    }
    const nextProvider = providerForImportedAgent(
      parsed.agentId,
      backendOptions.filter(canImportFromBackend).map((backend) => backend.provider),
    );
    if (nextProvider && nextProvider !== provider) {
      setProvider(nextProvider);
    }
    if (nextProvider === 'acp' || provider === 'acp') {
      setAgentId(parsed.agentId);
    }
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const parsed = parseSessionRef(sessionId);
    const normalizedSessionId = parsed.rawId;
    if (!normalizedSessionId) {
      setError(translate("workbench.sessionIDIsRequired"));
      return;
    }
    if (!canImportSelection) {
      setError(
        provider === 'acp'
          ? translate("workbench.chooseAReadyACPAgentBeforeImporting")
          : translate("workbench.chooseABackendThatSupportsLocalSession"),
      );
      return;
    }

    setBusy(true);
    setError(null);

    try {
      const imported = await importThread({
        sessionId: normalizedSessionId,
        provider,
        agentId: provider === 'acp' ? agentId : provider,
      });
      navigate(currentThreadHref(imported.thread.id));
    } catch (caught) {
      if (caught instanceof ApiError) {
        setError(caught.payload.message);
      } else {
        setError(caught instanceof Error ? caught.message : translate("workbench.unableToImportSession"));
      }
    } finally {
      setBusy(false);
    }
  }

  function cancelImport() {
    navigate(currentWorkspacesHref());
  }

  return (
    <div className="product-page" data-page="thread-import">
      <ProductHeader title={translate("workbench.importThreads")} backHref={currentWorkspacesHref()} backLabel={translate("workbench.backToWorkspaces")} />

      <header className="product-page-header">
        <div>
          <p className="product-eyebrow">{translate("workbench.sessionLibrary")}</p>
          <h1 className="product-title mt-1.5">{translate("workbench.importABackendSession")}</h1>
          <p className="product-description mt-2">
            {translate("workbench.resumeASessionDiscoveredOnThisSupervisor")}</p>
        </div>
      </header>

      <form className="divide-y divide-[var(--theme-border)]" onSubmit={handleSubmit}>
        <section className="product-divider-section space-y-5">
          {backendsError ? (
            <div className="host-error flex flex-col gap-3 rounded-md border px-4 py-3 text-sm sm:flex-row sm:items-center sm:justify-between" role="alert">
              <span>{translate("workbench.backendsCouldNotBeLoaded")} {backendsError}</span>
              <button
                className="host-secondary-button inline-flex min-h-11 shrink-0 items-center justify-center gap-2 rounded-md border px-3 text-xs font-semibold"
                onClick={() => setBackendLoadAttempt((attempt) => attempt + 1)}
                type="button"
              >
                <RefreshCw aria-hidden="true" className="h-3.5 w-3.5" />
                {translate("workbench.retry")}</button>
            </div>
          ) : null}

          <div>
          <label htmlFor="backend-provider" className="host-form-label text-sm font-medium">
            {translate("workbench.backend")}</label>
          <select
            id="backend-provider"
            value={provider}
            onChange={(event) => {
              setProvider(event.target.value as AgentBackendIdDto);
              setSessionId('');
              setError(null);
            }}
            disabled={busy || backendsLoading || backendOptions.length === 0}
            className="host-form-control mt-2 w-full rounded-md border px-3 outline-none transition"
          >
            {backendsLoading ? <option value="">{translate("workbench.loadingBackends")}</option> : null}
            {!backendsLoading && backendOptions.length === 0 ? <option value="">{translate("workbench.noBackendsAvailable")}</option> : null}
            {backendOptions.map((backend) => (
              <option disabled={!canImportFromBackend(backend)} key={backend.provider} value={backend.provider}>
                {backend.displayName}
                {canImportFromBackend(backend) ? '' : translate("workbench.importUnavailable")}
              </option>
            ))}
          </select>
          </div>
          {provider === 'acp' && (
            <div>
            <label htmlFor="acp-agent" className="host-form-label text-sm font-medium">
              {translate("workbench.aCPAgent")}</label>
            <select
              id="acp-agent"
              value={agentId ?? ''}
              onChange={(event) => {
                setAgentId(event.target.value || null);
                setSessionId('');
                setError(null);
              }}
              disabled={busy || agentsLoading || agents.length === 0}
              className="host-form-control mt-2 w-full rounded-md border px-3 outline-none transition"
            >
              <option value="">
                {agentsLoading ? translate("workbench.loadingAgents") : translate("workbench.noReadyACPAgent")}
              </option>
              {agents.map((agent) => (
                <option
                  key={agent.id}
                  value={agent.id}
                  disabled={agent.acpAgent?.availability !== 'ready'}
                >
                  {agent.displayName}
                  {agent.acpAgent?.availability === 'ready'
                    ? ''
                    : ` (${agent.acpAgent?.availability ?? translate("workbench.unavailable_1d5ee3")})`}
                </option>
              ))}
            </select>
            </div>
          )}
        </section>

        <section className="product-divider-section space-y-4">
          <div className="flex flex-col gap-1 sm:flex-row sm:items-end sm:justify-between">
            <div>
              <h2 className="text-sm font-semibold text-[var(--theme-fg)]">{translate("workbench.recentSessions")}</h2>
              <p className="mt-1 text-xs text-[var(--theme-fg-muted)]">
                {candidatesLoading ? translate("workbench.scanningThisSupervisor") : candidatesError ? translate("workbench.sessionScanFailed") : translate("workbench.ofSessions", { value1: filteredCandidates.length, value2: candidates.length })}
              </p>
            </div>
          </div>
          {candidates.length > 8 ? (
            <label className="relative block">
              <span className="sr-only">{translate("workbench.searchSessions")}</span>
              <Search aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-[var(--theme-fg-muted)]" />
              <input
                className="host-form-control w-full rounded-md border pl-9 pr-3 text-sm outline-none transition"
                disabled={busy}
                onChange={(event) => {
                  setCandidateQuery(event.target.value);
                  setError(null);
                }}
                placeholder={translate("workbench.searchTitleIDOrWorkspace")}
                type="search"
                value={candidateQuery}
              />
            </label>
          ) : null}
          <label className="block" htmlFor="available-session">
            <span className="host-form-label text-sm font-medium">{translate("workbench.availableSession")}</span>
          <select
            id="available-session"
            value={selectedCandidate?.sessionId ?? ''}
            onChange={(event) => {
              const nextSessionId = event.target.value;
              const candidate = filteredCandidates.find(
                (entry) => entry.sessionId === nextSessionId,
              );
              setSessionId(nextSessionId);
              const nextProvider = providerForImportedAgent(
                candidate?.agentId ?? null,
                backendOptions.filter(canImportFromBackend).map((backend) => backend.provider),
              );
              if (nextProvider && nextProvider !== provider) {
                setProvider(nextProvider);
              }
              setError(null);
            }}
            disabled={busy || candidatesLoading || filteredCandidates.length === 0}
            className="host-form-control mt-2 w-full rounded-md border px-3 outline-none transition"
          >
            <option value="">
              {candidatesLoading
                ? translate("workbench.loadingSessions")
                : filteredCandidates.length === 0
                  ? candidatesError
                    ? translate("workbench.sessionScanFailed")
                    : candidateQuery
                    ? translate("workbench.noMatchingSessions")
                    : translate("workbench.noUnmanagedSessionsFound")
                  : translate("workbench.selectASession")}
            </option>
            {filteredCandidates.map((candidate) => (
              <option key={candidate.sessionId} value={candidate.sessionId}>
                {candidate.title} · {candidate.sessionId}
              </option>
            ))}
          </select>
          </label>
          {selectedCandidate && (
            <div className="host-muted space-y-1 text-xs">
              <p className="break-all">{selectedCandidate.cwd}</p>
              {selectedCandidate.preview && (
                <p className="line-clamp-2">{selectedCandidate.preview}</p>
              )}
            </div>
          )}
          {/* A failed scan is not an empty list: say why, offer a rescan, and
              promise a manual import only when the Harness can import. */}
          {candidatesError && !backendsError && (
            <div className="host-error flex flex-col gap-3 rounded-md border px-4 py-3 text-sm sm:flex-row sm:items-center sm:justify-between" role="alert">
              <span>
                {translate("workbench.sessionScanFailedReason", { reason: candidatesError })}
                {canImportSelection ? ` ${translate("workbench.manualImportStillWorks")}` : ''}
              </span>
              <button
                className="host-secondary-button inline-flex min-h-11 shrink-0 items-center justify-center gap-2 rounded-md border px-3 text-xs font-semibold"
                onClick={() => setCandidateLoadAttempt((attempt) => attempt + 1)}
                disabled={candidatesLoading}
                type="button"
              >
                <RefreshCw aria-hidden="true" className="h-3.5 w-3.5" />
                {translate("workbench.rescanSessions")}</button>
            </div>
          )}
        </section>

        <section className="product-divider-section">
          <label htmlFor="session-id" className="host-form-label text-sm font-medium">
            {translate("workbench.sessionID")}</label>
          <input
            id="session-id"
            aria-describedby={error ? 'import-session-error' : undefined}
            aria-invalid={error ? true : undefined}
            disabled={busy}
            value={sessionId}
            onChange={(event) => applyPastedSession(event.target.value)}
            placeholder="codex://threads/01a0634a-23df-7191-acd2-1fca43a10418"
            className="host-form-control mt-2 w-full rounded-md border px-3 outline-none transition"
          />
          <p className="mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("workbench.fullCodexLinksAndSupportedBackendPrefixes")}</p>

          {error ? (
            <div className="host-error mt-4 rounded-md border px-4 py-3 text-sm" id="import-session-error" role="alert">
              {error}
            </div>
          ) : null}

          <div className="mt-6 flex flex-col-reverse gap-2 sm:flex-row">
            <button
              className="host-secondary-button min-h-11 rounded-md border px-5 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
              disabled={busy}
              onClick={cancelImport}
              type="button"
            >
              {translate("workbench.cancel")}</button>
            <button
              className="ui-action-primary min-h-11 rounded-md px-5 text-sm font-semibold transition disabled:cursor-not-allowed"
              disabled={busy || !parseSessionRef(sessionId).rawId || !canImportSelection}
              type="submit"
            >
              {busy ? translate("workbench.importing") : translate("workbench.importSession")}
            </button>
          </div>
        </section>
      </form>
    </div>
  );
}
