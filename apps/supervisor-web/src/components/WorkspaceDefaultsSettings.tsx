import { useEffect, useState } from 'react';
import { ErrorMark, PathChip } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { WorkspaceSettingsDto } from '../../../../packages/shared/src/index';
import { ApiError, fetchWorkspaceSettings, updateWorkspaceSettings } from '../lib/api';

/**
 * Where new projects go on one device. `deviceId` names the device every read
 * and save goes to (null: this Supervisor); the owner keys this panel by device,
 * so one device's values and draft are never shown or saved as another's.
 */
export function WorkspaceDefaultsSettings({ deviceId }: { deviceId: string | null }) {
  useI18n();
  const [workspaceSettings, setWorkspaceSettings] =
    useState<WorkspaceSettingsDto | null>(null);
  const [workspaceSettingsState, setWorkspaceSettingsState] = useState<{
    devHomeDraft: string;
    loading: boolean;
    saving: boolean;
    message: string | null;
    error: string | null;
  }>({
    devHomeDraft: '',
    loading: false,
    saving: false,
    message: null,
    error: null,
  });
  // Bumped by a Retry after a read that never arrived.
  const [workspaceRevision, setWorkspaceRevision] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setWorkspaceSettingsState((current) => ({
      ...current,
      loading: true,
      message: null,
      error: null,
    }));

    fetchWorkspaceSettings(deviceId)
      .then((settings) => {
        if (cancelled) {
          return;
        }

        setWorkspaceSettings(settings);
        setWorkspaceSettingsState((current) => ({
          ...current,
          devHomeDraft: settings.devHome,
          loading: false,
        }));
      })
      .catch((error) => {
        if (cancelled) {
          return;
        }

        setWorkspaceSettingsState((current) => ({
          ...current,
          loading: false,
          error:
            error instanceof ApiError
              ? error.message
              : translate("files.unableToLoadWorkspaceSettings"),
        }));
      });

    return () => {
      cancelled = true;
    };
  }, [deviceId, workspaceRevision]);

  async function handleSaveWorkspaceSettings() {
    const devHome = workspaceSettingsState.devHomeDraft.trim();
    if (!devHome || workspaceSettingsState.saving) {
      return;
    }

    setWorkspaceSettingsState((current) => ({
      ...current,
      saving: true,
      message: null,
      error: null,
    }));

    try {
      const updated = await updateWorkspaceSettings({
        devHome,
      }, deviceId);
      setWorkspaceSettings(updated);
      setWorkspaceSettingsState((current) => ({
        ...current,
        devHomeDraft: updated.devHome,
        saving: false,
        message: translate("files.workspaceDefaultsSaved"),
      }));
    } catch (error) {
      setWorkspaceSettingsState((current) => ({
        ...current,
        saving: false,
        error:
          error instanceof ApiError
            ? error.message
            : translate("files.unableToSaveWorkspaceSettings"),
      }));
    }
  }

  return (
    <section className="py-2">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold text-[var(--theme-fg)]">
            {translate("files.workspaceDefaults")}</h3>
          <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("files.gitProjectsCloneIntoDevHomeNew")}</p>
        </div>
      </div>
      {/* One column that never grows past the panel: long paths shorten. */}
      <div className="mt-3 grid grid-cols-[minmax(0,1fr)] gap-4">
        <div className="min-w-0 border-y border-[var(--theme-border)] py-3">
          <p className="text-xs font-medium text-[var(--theme-fg-muted)]">
            {translate("files.workspaceRoot")}</p>
          <div className="mt-1 flex min-w-0 items-center gap-1 text-xs leading-5 text-[var(--theme-fg-soft)]">
            {workspaceSettings?.workspaceRoot
              ? <PathChip path={workspaceSettings.workspaceRoot} />
              : workspaceSettingsState.loading ? translate("files.loading") : translate("files.unavailable")}
            {/* Nothing was read: the reason and a Retry on the mark. */}
            {!workspaceSettings && !workspaceSettingsState.loading && workspaceSettingsState.error ? (
              <ErrorMark message={workspaceSettingsState.error} onRetry={() => setWorkspaceRevision((value) => value + 1)} />
            ) : null}
          </div>
        </div>
        <div className="min-w-0">
          <label
            htmlFor="settings-dev-home"
            className="text-xs font-medium text-[var(--theme-fg-soft)]"
          >
            {translate("files.devHome")}</label>
          <div className="mt-1 flex flex-col gap-2 sm:flex-row">
            <input
              disabled={
                workspaceSettingsState.loading ||
                workspaceSettingsState.saving
              }
              id="settings-dev-home"
              value={workspaceSettingsState.devHomeDraft}
              onChange={(event) =>
                setWorkspaceSettingsState((current) => ({
                  ...current,
                  devHomeDraft: event.target.value,
                  message: null,
                  error: null,
                }))
              }
              // A preference: leaving the field or pressing Enter saves it.
              onBlur={() => {
                const draft = workspaceSettingsState.devHomeDraft.trim();
                if (draft && workspaceSettings && draft !== workspaceSettings.devHome) void handleSaveWorkspaceSettings();
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  void handleSaveWorkspaceSettings();
                }
              }}
              placeholder="/Users/name/dev"
              className="relay-input min-h-11 min-w-0 flex-1 rounded-md disabled:cursor-wait disabled:opacity-60"
            />
            <div className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                aria-label={translate("files.saveWorkspaceDefaults")}
                onClick={() => void handleSaveWorkspaceSettings()}
                disabled={
                  workspaceSettingsState.loading ||
                  workspaceSettingsState.saving ||
                  !workspaceSettingsState.devHomeDraft.trim()
                }
                className="relay-button-primary min-h-11 shrink-0 rounded-md px-4 max-sm:flex-1"
              >
                {workspaceSettingsState.saving ? translate("files.saving") : translate("files.save")}
              </button>
              {workspaceSettings && workspaceSettingsState.error ? (
                <ErrorMark message={workspaceSettingsState.error} onRetry={() => void handleSaveWorkspaceSettings()} />
              ) : null}
            </div>
          </div>
        </div>
      </div>
      {workspaceSettingsState.message ? (
        <p
          className="mt-3 rounded-md bg-[var(--status-success-bg)] px-3 py-2 text-xs text-[var(--status-success-fg)]"
          role="status"
        >
          {workspaceSettingsState.message}
        </p>
      ) : null}
    </section>
  );
}
