import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
import { FormDialog } from './FormDialog';
import { useFontSize } from '../lib/fontSize';
import { RuntimeManagement } from './RuntimeManagement';
import { LocalPasscodeSettings } from './LocalPasscodeSettings';
import { UpstreamsSettings } from './UpstreamsSettings';
import { HarnessResourceSettings } from './HarnessResourceSettings';
import { ModelPricingSettings } from './ModelPricingSettings';
import { ComposerShortcutSettings } from './ComposerShortcutSettings';
import { TourSettingsEntry } from './onboarding/TourProvider';
import { ThemePresetSettings } from './ThemePresetSettings';
import { useThemeRegistry } from '../lib/themePacks';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useHostScope } from '../lib/hostCapabilities';

import type {
  AgentBackendDto,
  AgentBackendIdDto,
  ProviderHostConfigArchiveDto,
  WorkspaceSettingsDto,
} from '../../../../packages/shared/src/index';
import {
  ApiError,
  applyProviderHostConfigArchive,
  createProviderHostConfigArchive,
  fetchAgentBackends,
  fetchProviderHostFile,
  fetchProviderHostConfigArchives,
  fetchWorkspaceSettings,
  renameProviderHostConfigArchive,
  updateProviderHostFile,
  updateWorkspaceSettings,
} from '../lib/api';
import {
  PathChip,
  PhoneHandChoice,
  useDevicePlugins,
  PluginSettingsButton,
  SettingsPanels,
  SettingsDialog,
  type SettingsSection,
} from '@pockymoe/thread-ui';
import { useAppShellNav } from './AppShellNavContext';
import {
  apiErrorMessage,
  defaultProviderHostFileState,
  fallbackBackends,
  fallbackManagementSchema,
  formatArchiveDate,
  normalizeBackendDescriptor,
  themeOptions,
} from './appShellNavigationModel';

// The Rust Supervisor answers every plugin import with 501 (`import_plugin_unsupported`
// in crates/supervisor/src/http.rs), so the form stays hidden until it can import.
const PLUGIN_IMPORT_SUPPORTED = false;

const UNAVAILABLE_REASONS: Record<string, TranslationKey> = {
  harness_missing: 'files.pluginHarnessMissing',
  unsupported_platform: 'files.pluginUnsupportedPlatform',
};

function unavailablePluginReason(plugin: unknown) {
  if (!plugin || typeof plugin !== 'object') {
    return null;
  }
  const availability = plugin as {
    available?: unknown;
    unavailableReason?: unknown;
    unavailableReasonCode?: unknown;
  };
  if (availability.available !== false) return null;
  const known = typeof availability.unavailableReasonCode === 'string' ? UNAVAILABLE_REASONS[availability.unavailableReasonCode] : undefined;
  if (known) return translate(known);
  return typeof availability.unavailableReason === 'string' ? availability.unavailableReason : null;
}

// The built-in Terminal plugin's manifest is English; show it in the reader's language.
function pluginDescription(plugin: { id: string; description?: string | null }) {
  return plugin.id === 'remote-codex.terminal' ? translate('files.terminalPluginDescription') : plugin.description;
}

export function AppShellSettingsDialog({
  embedded = false,
  section,
}: {
  embedded?: boolean;
  section?: string;
} = {}) {
  useI18n();
  const shellNav = useAppShellNav();
  // A host scope (Treer Agent UI) keeps only this browser's own preferences.
  const hostScope = useHostScope();
  // Global plugin management, even when this dialog opens inside a conversation.
  const plugins = useDevicePlugins();
  const [pluginImportDraft, setPluginImportDraft] = useState('');
  const [pluginImportState, setPluginImportState] = useState<{
    busy: boolean;
    message: string | null;
    error: string | null;
  }>({
    busy: false,
    message: null,
    error: null,
  });
  const [selectedFileName, setSelectedFileName] = useState<string | null>(null);
  const [files, setFiles] = useState<
    Record<
      string,
      {
        path: string;
        exists: boolean;
        originalContent: string;
        draftContent: string;
        loading: boolean;
        saving: boolean;
        error: string | null;
        saveMessage: string | null;
      }
    >
  >({});
  const selectedFile = selectedFileName ? files[selectedFileName] : null;
  const [archives, setArchives] = useState<ProviderHostConfigArchiveDto[]>([]);
  const [backends, setBackends] = useState<AgentBackendDto[]>(fallbackBackends);
  const [, setBackendState] = useState<{
    loading: boolean;
    saving: boolean;
    error: string | null;
    operatingProvider: AgentBackendIdDto | null;
    operatingAction: 'install' | 'update' | null;
    message: string | null;
  }>({
    loading: false,
    saving: false,
    error: null,
    operatingProvider: null,
    operatingAction: null,
    message: null,
  });
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
  const [archivesState, setArchivesState] = useState<{
    loading: boolean;
    creating: boolean;
    applyingId: string | null;
    renamingId: string | null;
    renamingBusyId: string | null;
    renameDraft: string;
    message: string | null;
    error: string | null;
  }>({
    loading: false,
    creating: false,
    applyingId: null,
    renamingId: null,
    renamingBusyId: null,
    renameDraft: '',
    message: null,
    error: null,
  });
  const selectedThemeMode = shellNav?.themeMode ?? 'system';
  // The applied pack may support one color mode only; the stored choice stays.
  const fixedThemeMode = useThemeRegistry().fixedMode;
  const settingsVisible = embedded || Boolean(shellNav?.settingsOpen);
  const shellNavRef = useRef(shellNav);
  shellNavRef.current = shellNav;

  const closeSettings = useCallback(() => {
    shellNavRef.current?.closeSettings();
    window.requestAnimationFrame(() => {
      document
        .querySelector<HTMLElement>(
          '[aria-controls="app-shell-navigation-menu"], [aria-haspopup="menu"]',
        )
        ?.focus();
    });
  }, []);
  const closeFileEditor = useCallback(() => {
    setSelectedFileName(null);
  }, []);

  async function handleImportPlugin() {
    const manifestJson = pluginImportDraft.trim();
    if (!manifestJson || pluginImportState.busy) {
      return;
    }

    setPluginImportState({
      busy: true,
      message: null,
      error: null,
    });
    try {
      await plugins.importPluginManifest({
        manifestJson,
        enabled: true,
      });
      setPluginImportDraft('');
      setPluginImportState({
        busy: false,
        message: translate("files.pluginManifestImported"),
        error: null,
      });
    } catch (error) {
      setPluginImportState({
        busy: false,
        message: null,
        error:
          error instanceof Error
            ? error.message
            : translate("files.unableToImportPluginManifest"),
      });
    }
  }
  const effectiveTheme = shellNav?.effectiveTheme ?? 'dark';
  const [fontSize, setFontSize] = useFontSize();
  const autoCollapseCompletedTurns =
    shellNav?.autoCollapseCompletedTurns ?? true;
  const [selectedBackend, setSelectedBackend] =
    useState<AgentBackendIdDto>('codex');
  const enabledPluginCount = plugins.plugins.filter(
    (plugin) => plugin.enabled,
  ).length;
  const pluginCountLabel = plugins.loading
    ? translate("files.loading")
    : translate('files.pluginsEnabledCount', { value1: enabledPluginCount, value2: plugins.plugins.length });
  const activeBackend =
    backends.find((backend) => backend.provider === selectedBackend) ??
    fallbackBackends.find((backend) => backend.provider === selectedBackend) ??
    fallbackBackends[0]!;
  const activeManagementSchema =
    activeBackend.managementSchema ??
    fallbackManagementSchema(activeBackend.provider);
  const editableFiles = activeManagementSchema.hostConfigFiles;

  useEffect(() => {
    if (!settingsVisible || section !== 'advanced') {
      return;
    }

    let cancelled = false;
    setBackendState((current) => ({
      ...current,
      loading: true,
      error: null,
    }));

    fetchAgentBackends()
      .then((records) => {
        if (cancelled) {
          return;
        }
        const merged = [
          ...records.map(normalizeBackendDescriptor),
          ...fallbackBackends.filter(
            (fallback) =>
              !records.some((record) => record.provider === fallback.provider),
          ),
        ];
        setBackends(merged);
        setBackendState((current) => ({
          ...current,
          loading: false,
        }));
      })
      .catch((error) => {
        if (cancelled) {
          return;
        }
        setBackends(fallbackBackends);
        setBackendState((current) => ({
          ...current,
          loading: false,
          error:
            error instanceof ApiError
              ? error.message
              : translate("files.unableToLoadBackendSettings"),
        }));
      });

    return () => {
      cancelled = true;
    };
  }, [settingsVisible, section]);

  useEffect(() => {
    if (!settingsVisible || section !== 'workspace') {
      return;
    }

    let cancelled = false;
    setWorkspaceSettingsState((current) => ({
      ...current,
      loading: true,
      message: null,
      error: null,
    }));

    fetchWorkspaceSettings()
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
  }, [settingsVisible, section]);

  useEffect(() => {
    if (
      !settingsVisible ||
      section !== 'advanced' ||
      !activeBackend.capabilities.management.hostConfigFiles
    ) {
      return;
    }

    let cancelled = false;

    async function loadFiles() {
      setFiles((current) => {
        const next = { ...current };
        for (const file of editableFiles) {
          next[file.name] = {
            ...defaultProviderHostFileState(file.name),
            ...current[file.name],
            loading: true,
            saving: false,
            error: null,
            saveMessage: null,
          };
        }
        return next;
      });

      const results = await Promise.allSettled(
        editableFiles.map(async (file) => ({
          name: file.name,
          result: await fetchProviderHostFile(
            activeBackend.provider,
            file.name,
          ),
        })),
      );

      if (cancelled) {
        return;
      }

      setFiles((current) => {
        const next = { ...current };

        for (const result of results) {
          if (result.status === 'fulfilled') {
            const { name, result: fileResult } = result.value;
            next[name] = {
              path: fileResult.path,
              exists: fileResult.exists,
              originalContent: fileResult.content,
              draftContent: fileResult.content,
              loading: false,
              saving: false,
              error: null,
              saveMessage: null,
            };
            continue;
          }

          const message =
            result.reason instanceof ApiError
              ? result.reason.message
              : translate("files.unableToLoadTheFile");
          const failedName =
            editableFiles[results.indexOf(result)]?.name ??
            editableFiles[0]?.name;
          if (!failedName) {
            continue;
          }
          next[failedName] = {
            ...defaultProviderHostFileState(failedName),
            ...next[failedName],
            loading: false,
            saving: false,
            error: message,
            saveMessage: null,
          };
        }

        return next;
      });
    }

    void loadFiles();

    return () => {
      cancelled = true;
    };
  }, [
    activeBackend.capabilities.management.hostConfigFiles,
    activeBackend.provider,
    editableFiles,
    settingsVisible,
    section,
  ]);

  useEffect(() => {
    if (!settingsVisible || section !== 'advanced') {
      return;
    }

    let cancelled = false;

    async function loadArchives() {
      setArchivesState((current) => ({
        ...current,
        loading: true,
        error: null,
        message: null,
      }));

      try {
        const results = await fetchProviderHostConfigArchives(
          activeBackend.provider,
        );
        if (cancelled) {
          return;
        }

        setArchives(results);
        setArchivesState((current) => ({
          ...current,
          loading: false,
        }));
      } catch (error) {
        if (cancelled) {
          return;
        }

        setArchivesState((current) => ({
          ...current,
          loading: false,
          error:
            error instanceof ApiError
              ? error.message
              : translate("files.unableToLoadConfigArchives"),
        }));
      }
    }

    void loadArchives();

    return () => {
      cancelled = true;
    };
  }, [
    activeBackend.provider,
    activeManagementSchema.configArchives,
    settingsVisible,
    section,
  ]);

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
      });
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

  async function handleSave(name: string) {
    const fileState = files[name];
    if (!fileState || fileState.saving) {
      return;
    }

    setFiles((current) => ({
      ...current,
      [name]: {
        ...defaultProviderHostFileState(name),
        ...current[name],
        saving: true,
        error: null,
        saveMessage: null,
      },
    }));

    try {
      const updated = await updateProviderHostFile(
        activeBackend.provider,
        name,
        {
          content: fileState.draftContent,
        },
      );

      setFiles((current) => ({
        ...current,
        [name]: {
          path: updated.path,
          exists: updated.exists,
          originalContent: updated.content,
          draftContent: updated.content,
          loading: false,
          saving: false,
          error: null,
          saveMessage: translate("files.saved_c0ae8f"),
        },
      }));
    } catch (error) {
      setFiles((current) => ({
        ...current,
        [name]: {
          ...defaultProviderHostFileState(name),
          ...current[name],
          saving: false,
          error:
            error instanceof ApiError
              ? error.message
              : translate("files.unableToSaveTheFile"),
          saveMessage: null,
        },
      }));
    }
  }

  async function handleCreateArchive() {
    if (
      archivesState.creating ||
      archivesState.applyingId !== null ||
      archivesState.renamingBusyId !== null
    ) {
      return;
    }

    setArchivesState((current) => ({
      ...current,
      creating: true,
      message: null,
      error: null,
    }));

    try {
      const archive = await createProviderHostConfigArchive(
        activeBackend.provider,
      );
      setArchives((current) => [archive, ...current]);
      setArchivesState((current) => ({
        ...current,
        creating: false,
        message: translate("files.backupCreated"),
      }));
    } catch (error) {
      setArchivesState((current) => ({
        ...current,
        creating: false,
        error:
          error instanceof ApiError
            ? error.message
            : translate("files.unableToCreateAConfigBackup"),
      }));
    }
  }

  async function handleApplyArchive(archive: ProviderHostConfigArchiveDto) {
    if (
      archivesState.applyingId ||
      archivesState.creating ||
      archivesState.renamingBusyId !== null
    ) {
      return;
    }

    setArchivesState((current) => ({
      ...current,
      applyingId: archive.id,
      message: null,
      error: null,
    }));

    try {
      const result = await applyProviderHostConfigArchive(
        activeBackend.provider,
        archive.id,
      );
      setArchivesState((current) => ({
        ...current,
        applyingId: null,
        message:
          result.status.state === 'ready'
            ? translate("files.appliedAndRestarted", { value1: result.archive.label, value2: activeBackend.displayName })
            : translate("files.appliedState", { value1: result.archive.label, value2: activeBackend.displayName, value3: result.status.state }),
      }));
    } catch (error) {
      setArchivesState((current) => ({
        ...current,
        applyingId: null,
        error:
          error instanceof ApiError
            ? error.message
            : translate("files.unableToApplyTheConfigArchive"),
      }));
    }
  }

  async function handleRenameArchive(archive: ProviderHostConfigArchiveDto) {
    const label = archivesState.renameDraft.trim();
    if (
      !label ||
      archivesState.renamingId !== archive.id ||
      archivesState.renamingBusyId !== null ||
      archivesState.creating ||
      archivesState.applyingId !== null
    ) {
      return;
    }

    setArchivesState((current) => ({
      ...current,
      renamingBusyId: archive.id,
      message: null,
      error: null,
    }));

    try {
      const updated = await renameProviderHostConfigArchive(
        activeBackend.provider,
        archive.id,
        { label },
      );
      setArchives((current) =>
        current.map((entry) => (entry.id === archive.id ? updated : entry)),
      );
      setArchivesState((current) => ({
        ...current,
        renamingId: null,
        renamingBusyId: null,
        renameDraft: '',
        message: translate("files.backupRenamed"),
      }));
    } catch (error) {
      setArchivesState((current) => ({
        ...current,
        renamingBusyId: null,
        error:
          error instanceof ApiError
            ? error.message
            : translate("files.unableToRenameTheConfigBackup"),
      }));
    }
  }

  if (!settingsVisible) {
    return null;
  }

  const pluginsManagementNode = (
    <>
      <div className="mt-3 divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
        {plugins.plugins.map((plugin) => {
          const unavailable = unavailablePluginReason(plugin);
          const toggleId = `plugin-enabled-${plugin.id}`;
          return (
            // The gear is a sibling of the label so opening settings never toggles the plugin.
            <div key={plugin.id} className="flex min-h-11 items-start gap-2 py-3">
              <label htmlFor={toggleId} className="min-w-0 flex-1 cursor-pointer">
                <span className="block text-sm font-medium text-[var(--theme-fg)]">
                  {plugin.name}
                </span>
                <span className="settings-hint mt-1 block text-xs leading-5 text-[var(--theme-fg-muted)]">
                  {pluginDescription(plugin)}
                </span>
                <span className="settings-hint mt-1 block text-[11px] leading-5 text-[var(--theme-fg-muted)]">
                  {[
                    [
                      ...plugin.capabilities.artifactTypes.map((type) => type.type),
                      ...plugin.capabilities.threadPanels.map(
                        (panel) => panel.kind ?? panel.id,
                      ),
                    ].join(', ') || translate("files.utility"),
                    plugin.source === 'imported'
                      ? translate("files.importedManifest")
                      : translate("files.builtInModule"),
                  ].join(' · ')}
                </span>
                {unavailable ? (
                  <span className="mt-1 block text-xs leading-5 text-[var(--status-warning-fg)]">
                    {unavailable}
                  </span>
                ) : null}
              </label>
              <PluginSettingsButton plugin={plugin} />
              <span className="grid h-8 w-8 shrink-0 place-items-center">
                <input
                  id={toggleId}
                  className="h-5 w-5 accent-[var(--theme-accent-solid)] disabled:cursor-not-allowed disabled:opacity-50"
                  checked={plugin.enabled}
                  disabled={unavailable !== null}
                  aria-label={translate("files.enabled", { value1: plugin.name })}
                  onChange={(event) =>
                    void plugins.setPluginEnabled(
                      plugin.id,
                      event.currentTarget.checked,
                    )
                  }
                  type="checkbox"
                />
              </span>
            </div>
          );
        })}
        {plugins.plugins.length === 0 && (
          <p className="py-4 text-xs text-[var(--theme-fg-muted)]">
            {translate("files.noPluginsAreRegistered")}</p>
        )}
      </div>
      {PLUGIN_IMPORT_SUPPORTED && <div className="mt-3 border-t border-[var(--theme-border)] pt-3">
        <label className="block text-xs font-medium text-[var(--theme-fg)]">
          {translate("files.importManifestJSON")}</label>
        <textarea
          disabled={pluginImportState.busy}
          value={pluginImportDraft}
          onChange={(event) => {
            setPluginImportDraft(event.currentTarget.value);
            if (pluginImportState.message || pluginImportState.error) {
              setPluginImportState({
                busy: false,
                message: null,
                error: null,
              });
            }
          }}
          placeholder='{"id":"example.viewer","name":"Example Viewer","version":"0.1.0",...}'
          rows={4}
          className="mt-2 min-h-28 w-full resize-y rounded-md border border-[var(--theme-border-strong)] bg-[var(--theme-surface-strong)] px-3 py-2 font-mono text-xs leading-5 text-[var(--theme-fg)] outline-none transition placeholder:text-[var(--theme-fg-muted)] focus-visible:border-[var(--theme-accent-border)] focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] disabled:cursor-wait disabled:opacity-60"
        />
        <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
          <p className="settings-hint max-w-[42rem] text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate("files.importsRegisterManifestDeclaredArtifactTypesRendering")}</p>
          <button
            type="button"
            onClick={() => void handleImportPlugin()}
            disabled={!pluginImportDraft.trim() || pluginImportState.busy}
            className="host-secondary-button min-h-11 shrink-0 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
          >
            {pluginImportState.busy ? translate("files.importing") : translate("files.import")}
          </button>
        </div>
        {pluginImportState.error && (
          <p
            className="host-error mt-2 rounded-md border px-3 py-2 text-xs"
            role="alert"
          >
            {pluginImportState.error}
          </p>
        )}
        {pluginImportState.message && (
          <p
            className="mt-2 rounded-md bg-[var(--status-success-bg)] px-3 py-2 text-xs text-[var(--status-success-fg)]"
            role="status"
          >
            {pluginImportState.message}
          </p>
        )}
      </div>}
      {plugins.error && (
        <p
          className="host-error mt-2 rounded-md border px-3 py-2 text-xs"
          role="alert"
        >
          {plugins.error}
        </p>
      )}
    </>
  );

  const settingsContentNode = (
    <>
      <div
        className={`min-h-0 flex-1 overflow-y-auto ${embedded ? '!overflow-visible !flex-none p-0' : 'px-4 pb-[max(1.5rem,env(safe-area-inset-bottom))] sm:px-5'}`}
      >
        <div className="divide-y divide-[var(--theme-border)]">
          {section === 'preferences' && <fieldset className="py-5"><LanguageSwitcher /><p className="settings-hint mt-1 text-xs text-[var(--theme-fg-muted)]">{translate("files.chooseTheInterfaceLanguageForThisBrowser")}</p></fieldset>}
          {section === 'preferences' && !hostScope && <ComposerShortcutSettings />}
          {section === 'preferences' && !hostScope && <TourSettingsEntry />}
          {section === 'preferences' ? (
            <fieldset className="py-5">
              <legend className="text-sm font-semibold text-[var(--theme-fg)]">
                {translate("files.appearance")}</legend>
              <ThemePresetSettings />
              <p className="mt-5 flex items-center gap-2 text-xs font-semibold text-[var(--theme-fg)]">
                {translate("files.colorMode")}
                {fixedThemeMode ? (
                  <span className="theme-mode-fixed" data-testid="theme-mode-fixed">
                    {translate(fixedThemeMode === 'light' ? "files.themeLightOnly" : "files.themeDarkOnly")}
                  </span>
                ) : null}
              </p>
              <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
                {translate("files.colorModeShowing", { value1: translate(effectiveTheme === 'light' ? 'files.light' : 'files.dark') })}
              </p>
              <div className="product-segmented mt-3 grid w-full grid-cols-3 sm:w-auto">
                {themeOptions.map((option) => {
                  return (
                    <label
                      className="product-segment min-h-11 flex-1 cursor-pointer"
                      key={option.value}
                    >
                      <input
                        checked={(fixedThemeMode ?? selectedThemeMode) === option.value}
                        className="sr-only"
                        disabled={Boolean(fixedThemeMode)}
                        name="settings-theme"
                        onChange={() => shellNav?.setThemeMode(option.value)}
                        type="radio"
                        value={option.value}
                      />
                      <span>{option.label}</span>
                    </label>
                  );
                })}
              </div>
              {fixedThemeMode ? null : <p className="settings-hint mt-2 text-xs leading-5 text-[var(--theme-fg-muted)]">
                {
                  themeOptions.find(
                    (option) => option.value === selectedThemeMode,
                  )?.description
                }
              </p>}
            </fieldset>
          ) : null}

          {section === 'preferences' && (
            <section className="py-5">
              <label htmlFor="settings-font-size" className="block text-sm font-semibold">{translate("files.textSize")}</label>
              <p className="settings-hint mt-1 text-xs text-[var(--theme-fg-muted)]">{translate("files.adjustConversationComposerAndActivityTextChanges")}</p>
              <div className="mt-3 flex items-center gap-4">
                <input id="settings-font-size" aria-label={translate("files.textSize")} type="range" min="12" max="22" step="1" value={fontSize} onChange={(event) => setFontSize(Number(event.currentTarget.value))} className="w-48 accent-[var(--theme-accent-solid)]" />
                <output htmlFor="settings-font-size" className="text-sm tabular-nums">{fontSize}px</output>
                <button type="button" onClick={() => setFontSize(16)} className="relay-button-secondary">{translate("files.reset")}</button>
              </div>
            </section>
          )}
          {section === 'preferences' && shellNav?.setPhoneDock && (
            <section className="py-5">
              <label className="flex min-h-11 items-center justify-between gap-4">
                <span>
                  <span className="block text-sm font-semibold">{translate("workbench.phoneDockSetting")}</span>
                  <span className="settings-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">{translate("workbench.phoneDockSettingHint")}</span>
                </span>
                <input
                  type="checkbox"
                  aria-label={translate("workbench.phoneDockSettingToggle")}
                  checked={shellNav.phoneDock ?? true}
                  onChange={(e) => shellNav.setPhoneDock?.(e.currentTarget.checked)}
                  className="h-5 w-5 accent-[var(--theme-accent-solid)]"
                />
              </label>
              {shellNav.setPhoneHand && (shellNav.phoneDock ?? true) && (
                <PhoneHandChoice hand={shellNav.phoneHand ?? 'right'} onChange={shellNav.setPhoneHand} />
              )}
            </section>
          )}
          {section === 'preferences' &&
            shellNav?.setAutoCollapseCompletedTurns && (
              <section className="py-5">
                <label className="flex min-h-11 items-center justify-between gap-4">
                  <span>
                    <span className="block text-sm font-semibold">
                      {translate("files.threadTimeline")}</span>
                    <span className="settings-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">
                      {translate("files.collapseCompletedTurnsToKeepConversationsEasy")}</span>
                  </span>
                  <input
                    type="checkbox"
                    aria-label={translate("files.autoCollapse")}
                    checked={autoCollapseCompletedTurns}
                    onChange={(e) =>
                      shellNav.setAutoCollapseCompletedTurns?.(
                        e.currentTarget.checked,
                      )
                    }
                    className="h-5 w-5 accent-[var(--theme-accent-solid)]"
                  />
                </label>
              </section>
            )}
          {section === 'plugins' && (
            <section className="py-2">
              <div className="flex items-center justify-between text-xs text-[var(--theme-fg-muted)]">
                <span>{pluginCountLabel}</span>
                <button
                  className="host-secondary-button rounded-md border px-3 py-2"
                  onClick={() => void plugins.refresh()}
                >
                  {translate("files.refreshPlugins")}</button>
              </div>
              {pluginsManagementNode}
            </section>
          )}
          {section === 'preferences' && (
            <section className="py-5">
              <label className="flex min-h-11 items-center justify-between gap-4">
                <span>
                  <span className="block text-sm font-semibold">
                    {translate("files.showAgentStatusSummaries")}</span>
                  <span className="settings-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">
                    {translate("files.showIntermediateThinkingSummariesWithTheirOwn")}</span>
                </span>
                <input
                  type="checkbox"
                  checked={shellNav?.showReasoningSummaries ?? false}
                  onChange={(event) =>
                    shellNav?.setShowReasoningSummaries?.(
                      event.currentTarget.checked,
                    )
                  }
                  className="h-5 w-5 accent-[var(--theme-accent-solid)]"
                />
              </label>
            </section>
          )}
          {section === 'preferences' && !hostScope && (
            <details className="settings-detail">
              <summary>{translate("files.modelPricing")}</summary>
              <div>
                <ModelPricingSettings />
              </div>
            </details>
          )}

          {section === 'workspace' && (
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
                  <div className="mt-1 flex min-w-0 text-xs leading-5 text-[var(--theme-fg-soft)]">
                    {workspaceSettings?.workspaceRoot
                      ? <PathChip path={workspaceSettings.workspaceRoot} />
                      : workspaceSettingsState.loading ? translate("files.loading") : translate("files.unavailable")}
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
                    <button
                      type="button"
                      aria-label={translate("files.saveWorkspaceDefaults")}
                      onClick={() => void handleSaveWorkspaceSettings()}
                      disabled={
                        workspaceSettingsState.loading ||
                        workspaceSettingsState.saving ||
                        !workspaceSettingsState.devHomeDraft.trim()
                      }
                      className="relay-button-primary min-h-11 shrink-0 rounded-md px-4"
                    >
                      {workspaceSettingsState.saving ? translate("files.saving") : translate("files.save")}
                    </button>
                  </div>
                </div>
              </div>
              {workspaceSettingsState.error ? (
                <p
                  className="host-error mt-3 rounded-md border px-3 py-2 text-xs"
                  role="alert"
                >
                  {workspaceSettingsState.error}
                </p>
              ) : workspaceSettingsState.message ? (
                <p
                  className="mt-3 rounded-md bg-[var(--status-success-bg)] px-3 py-2 text-xs text-[var(--status-success-fg)]"
                  role="status"
                >
                  {workspaceSettingsState.message}
                </p>
              ) : null}
            </section>
          )}
          {section === 'upstreams' && <UpstreamsSettings />}
          {section === 'resources' && <HarnessResourceSettings />}
          {section === 'device' && !hostScope && <LocalPasscodeSettings />}
          {(section === 'device' || section === 'harnesses') && (
            <RuntimeManagement view={section === 'device' && hostScope ? 'supervisor' : section} />
          )}

          {section === 'advanced' && (
            <>
              <label className="block text-xs font-medium">
                {translate("files.harnessConfiguration")}<select
                  className="host-input mt-2 w-full rounded-lg border p-3 text-sm"
                  value={selectedBackend}
                  onChange={(event) => {
                    setSelectedBackend(event.target.value as AgentBackendIdDto);
                    setSelectedFileName(null);
                  }}
                >
                  {backends
                    .filter((b) => b.capabilities.management.hostConfigFiles)
                    .map((b) => (
                      <option key={b.provider} value={b.provider}>
                        {b.displayName}
                      </option>
                    ))}
                </select>
              </label>

              <section className="py-5">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h3 className="text-sm font-semibold text-[var(--theme-fg)]">
                      {translate("files.providerHostFiles")}</h3>
                    <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
                      {activeBackend.displayName} {translate("files.exposesTheseEditableFilesThroughItsBackend")}</p>
                  </div>
                </div>
                <div className="mt-3 divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
                  {editableFiles.map((file) => {
                    const state = files[file.name] ?? {
                      path: file.name,
                      exists: false,
                      originalContent: '',
                      draftContent: '',
                      loading: false,
                      saving: false,
                      error: null,
                      saveMessage: null,
                    };
                    const dirty = state.draftContent !== state.originalContent;

                    return (
                      <button
                        key={file.name}
                        type="button"
                        onClick={() => setSelectedFileName(file.name)}
                        className="block min-h-11 w-full px-2 py-3 text-left transition hover:bg-[var(--theme-hover)] focus-visible:outline-none focus-visible:bg-[var(--theme-hover)] focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-accent-ring)]"
                      >
                        <div className="flex items-start justify-between gap-3">
                          <div className="min-w-0">
                            <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
                              {file.label}
                            </p>
                            <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
                              {file.description}
                            </p>
                            {state.error ? (
                              <p
                                className="mt-1 text-xs text-[var(--status-danger-fg)]"
                                role="alert"
                              >
                                {state.error}
                              </p>
                            ) : null}
                          </div>
                          <div className="shrink-0">
                            {state.loading ? (
                              <span className="text-[11px] font-medium text-[var(--theme-fg-muted)]">
                                {translate("files.loading_8f26c6")}</span>
                            ) : dirty ? (
                              <span className="text-[11px] font-medium text-[var(--theme-accent-strong)]">
                                {translate("files.unsaved")}</span>
                            ) : state.exists ? (
                              <span className="text-[11px] font-medium text-[var(--status-success-fg)]">
                                {translate("files.ready")}</span>
                            ) : (
                              <span className="text-[11px] font-medium text-[var(--status-info-fg)]">
                                {translate("files.new")}</span>
                            )}
                          </div>
                        </div>
                      </button>
                    );
                  })}
                  {editableFiles.length === 0 ? (
                    <p className="py-4 text-xs text-[var(--theme-fg-muted)]">
                      {translate("files.thisBackendDoesNotExposeEditableHost")}</p>
                  ) : null}
                </div>
              </section>

              {activeManagementSchema.configArchives ? (
                <details className="settings-detail">
                  <summary>{translate("files.configurationBackups")}</summary>
                  <div>
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                      <div className="min-w-0">
                        <h3 className="text-sm font-semibold text-[var(--theme-fg)]">
                          {translate("files.configArchives")}</h3>
                        <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
                          {translate("files.backupTheSelectedBackendHostFilesThen")}</p>
                      </div>
                      <button
                        type="button"
                        onClick={() => void handleCreateArchive()}
                        disabled={
                          archivesState.creating ||
                          archivesState.applyingId !== null ||
                          archivesState.renamingBusyId !== null
                        }
                        className="host-secondary-button min-h-11 shrink-0 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {archivesState.creating
                          ? translate("files.creating")
                          : translate("files.createBackup")}
                      </button>
                    </div>
                    {archivesState.error ? (
                      <p
                        className="host-error mt-3 rounded-md border px-3 py-2 text-xs"
                        role="alert"
                      >
                        {archivesState.error}
                      </p>
                    ) : archivesState.message ? (
                      <p
                        className="mt-3 rounded-md bg-[var(--status-success-bg)] px-3 py-2 text-xs text-[var(--status-success-fg)]"
                        role="status"
                      >
                        {archivesState.message}
                      </p>
                    ) : null}
                    <div className="mt-3 divide-y divide-[var(--theme-border)] border-y border-[var(--theme-border)]">
                      {archivesState.loading ? (
                        <p
                          className="py-4 text-xs text-[var(--theme-fg-muted)]"
                          role="status"
                        >
                          {translate("files.loadingBackups")}</p>
                      ) : archives.length === 0 ? (
                        <p className="py-4 text-xs text-[var(--theme-fg-muted)]">
                          {translate("files.noConfigBackupsYet")}</p>
                      ) : (
                        archives.map((archive) => {
                          const renaming =
                            archivesState.renamingId === archive.id;
                          return (
                            <div key={archive.id} className="py-3">
                              <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                                <div className="min-w-0">
                                  {renaming ? (
                                    <div className="flex max-w-xl flex-col gap-2 sm:flex-row">
                                      <input
                                        aria-label={translate("files.rename", { value1: archive.label })}
                                        disabled={
                                          archivesState.renamingBusyId ===
                                          archive.id
                                        }
                                        value={archivesState.renameDraft}
                                        onChange={(event) =>
                                          setArchivesState((current) => ({
                                            ...current,
                                            renameDraft: event.target.value,
                                            error: null,
                                            message: null,
                                          }))
                                        }
                                        className="relay-input min-h-11 min-w-0 flex-1 rounded-md disabled:cursor-wait disabled:opacity-60"
                                      />
                                      <button
                                        type="button"
                                        aria-label={translate("files.saveArchiveName", { value1: archive.label })}
                                        onClick={() =>
                                          void handleRenameArchive(archive)
                                        }
                                        disabled={
                                          archivesState.renamingBusyId ===
                                            archive.id ||
                                          !archivesState.renameDraft.trim()
                                        }
                                        className="relay-button-primary min-h-11 rounded-md px-3"
                                      >
                                        {archivesState.renamingBusyId ===
                                        archive.id
                                          ? translate("files.saving")
                                          : translate("files.save")}
                                      </button>
                                      <button
                                        type="button"
                                        onClick={() =>
                                          setArchivesState((current) => ({
                                            ...current,
                                            renamingId: null,
                                            renameDraft: '',
                                          }))
                                        }
                                        disabled={
                                          archivesState.renamingBusyId ===
                                          archive.id
                                        }
                                        className="host-secondary-button min-h-11 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
                                      >
                                        {translate("files.cancel")}</button>
                                    </div>
                                  ) : (
                                    <p className="truncate text-sm font-medium text-[var(--theme-fg)]">
                                      {archive.label}
                                    </p>
                                  )}
                                  <div className="mt-2 flex flex-wrap items-center gap-2 text-[11px] text-[var(--theme-fg-muted)]">
                                    <span>
                                      {translate("files.created")}{' '}
                                      {formatArchiveDate(archive.createdAt)}
                                    </span>
                                    {editableFiles.map((file) => (
                                      <span
                                        key={file.name}
                                        className="font-mono"
                                      >
                                        {file.name}:{' '}
                                        {archive.files[
                                          file.name as keyof typeof archive.files
                                        ]?.exists
                                          ? translate("files.saved")
                                          : translate("files.missing")}
                                      </span>
                                    ))}
                                  </div>
                                </div>
                                <div className="flex shrink-0 flex-wrap gap-2">
                                  <button
                                    type="button"
                                    onClick={() =>
                                      setArchivesState((current) => ({
                                        ...current,
                                        renamingId: archive.id,
                                        renameDraft: archive.label,
                                        message: null,
                                        error: null,
                                      }))
                                    }
                                    disabled={
                                      renaming ||
                                      archivesState.creating ||
                                      archivesState.renamingBusyId !== null ||
                                      archivesState.applyingId !== null
                                    }
                                    className="host-secondary-button min-h-11 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
                                  >
                                    {translate("files.rename_d3f4cb")}</button>
                                  <button
                                    type="button"
                                    onClick={() =>
                                      void handleApplyArchive(archive)
                                    }
                                    disabled={
                                      archivesState.applyingId !== null ||
                                      archivesState.creating ||
                                      archivesState.renamingBusyId !== null
                                    }
                                    className="host-secondary-button min-h-11 rounded-md border px-3 text-xs font-medium transition disabled:cursor-not-allowed disabled:opacity-50"
                                  >
                                    {archivesState.applyingId === archive.id
                                      ? translate("files.applying")
                                      : translate("files.apply")}
                                  </button>
                                </div>
                              </div>
                            </div>
                          );
                        })
                      )}
                    </div>
                  </div>
                </details>
              ) : null}
            </>
          )}
        </div>
      </div>
    </>
  );

  if (embedded) {
    return (
      <div className="min-w-0">
        {section ? (
          settingsContentNode
        ) : (
          <SettingsPanels sections={appSettingsSections()} />
        )}
        {selectedFileName && selectedFile && (
          <FormDialog
            title={selectedFileName}
            onClose={closeFileEditor}
            busy={selectedFile.saving}
          >
            <p className="break-all font-mono text-xs text-[var(--theme-fg-muted)]">
              {selectedFile.path}
            </p>
            <textarea
              aria-label={translate("files.edit", { value1: selectedFileName })}
              disabled={selectedFile.loading || selectedFile.saving}
              spellCheck={false}
              className="host-input my-3 min-h-64 w-full rounded-lg border p-3 font-mono text-xs"
              value={selectedFile.draftContent}
              onChange={(e) =>
                setFiles((current) => ({
                  ...current,
                  [selectedFileName]: {
                    ...current[selectedFileName]!,
                    draftContent: e.target.value,
                  },
                }))
              }
            />
            {selectedFile.error && <p role="alert">{selectedFile.error}</p>}
            {selectedFile.saveMessage && (
              <p role="status">{selectedFile.saveMessage}</p>
            )}
            <button
              className="relay-button-primary min-h-10"
              disabled={
                selectedFile.loading ||
                selectedFile.saving ||
                selectedFile.draftContent === selectedFile.originalContent
              }
              onClick={() => void handleSave(selectedFileName)}
            >
              {translate("files.saveFile")}</button>
          </FormDialog>
        )}
      </div>
    );
  }

  return <SettingsDialog open
    onOpenChange={(open) => { if (!open) closeSettings(); }}
    themeMode={selectedThemeMode} effectiveTheme={effectiveTheme}
    sections={appSettingsSections()}
    {...(shellNav?.settingsSection ? { initialId: shellNav.settingsSection } : {})}
    contentProps={{ 'data-testid': 'settingsDialog', onCloseAutoFocus: (event) => event.preventDefault() }}
  />;

}

/** Plugin settings open from the gear on their row in Plugins, not as top-level tabs.
 * A host scope (Treer Agent UI) gets Preferences and Device, which there holds only
 * hub maintenance (Check/Update/Restart); the other tabs administer the device. */
export function appSettingsSections({ scoped = false }: { scoped?: boolean } = {}): SettingsSection[] {
  const sections = [
    {
      id: 'preferences',
      label: translate("files.preferences"),
      description: translate("files.appearanceAndConversationDisplayTailoredToYou"),
    },
    {
      id: 'harnesses',
      label: translate("files.harnesses"),
      description:
        translate('settings.harnessMaintenanceDescription'),
    },
    {
      id: 'upstreams',
      label: translate('settings.upstreamsTab'),
      description: translate('settings.upstreamsDescription'),
    },
    {
      id: 'resources',
      label: translate('settings.resourcesTab'),
      description: translate('settings.resourcesDescription'),
    },
    {
      id: 'device',
      label: translate("files.device"),
      description: translate("files.supervisorMaintenanceAndReusableDeviceTemplates"),
    },
    {
      id: 'workspace',
      label: translate("files.workspace"),
      description: translate("files.defaultLocationsForProjectsOnThisDevice"),
    },
    {
      id: 'plugins',
      label: translate("files.plugins"),
      description: translate("files.renderersAndExtensionsForYourWorkspace"),
    },
    {
      id: 'advanced',
      label: translate("files.advanced"),
      description: translate("files.nativeConfigurationFilesAndRecoveryBackups"),
    },
  ];
  return sections.filter((section) => !scoped || section.id === 'preferences' || section.id === 'device').map((section) => ({
    ...section,
    ...(scoped && section.id === 'device' ? { description: translate('settings.treerDeviceDescription') } : {}),
  })).map((section) => ({
    ...section,
    content: <AppShellSettingsDialog embedded section={section.id} />,
  }));
}
