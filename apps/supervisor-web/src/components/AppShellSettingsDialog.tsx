import { NotificationSettings } from './NotificationSettings';
import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
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
  WorkspaceSettingsDto,
} from '../../../../packages/shared/src/index';
import {
  ApiError,
  relayModeActive,
  fetchWorkspaceSettings,
  updateWorkspaceSettings,
} from '../lib/api';
import {
  ErrorMark,
  PathChip,
  PhoneHandChoice,
  useDevicePlugins,
  PluginSettingsButton,
  SettingsPanels,
  SettingsDialog,
  type SettingsSection,
} from '@pockymoe/thread-ui';
import { useAppShellNav } from './AppShellNavContext';
import { themeOptions } from './appShellNavigationModel';

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
  const enabledPluginCount = plugins.plugins.filter(
    (plugin) => plugin.enabled,
  ).length;
  const pluginCountLabel = plugins.loading
    ? translate("files.loading")
    : translate('files.pluginsEnabledCount', { value1: enabledPluginCount, value2: plugins.plugins.length });
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
  }, [settingsVisible, section, workspaceRevision]);

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
            <div key={plugin.id} className="py-3">
            <div className="flex min-h-11 items-start gap-2">
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
                      : plugin.source === 'external'
                        ? translate("files.installedFromGit")
                        : translate("files.builtInModule"),
                  ].join(' · ')}
                </span>
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
              {/* A plugin this device cannot run keeps its switch, greyed, with the reason on a mark. */}
              {unavailable ? (
                <span className="grid h-8 shrink-0 place-items-center">
                  <ErrorMark kind="unavailable" message={unavailable} />
                </span>
              ) : null}
            </div>
            {/* An enabled plugin says whether conversations that follow the default have it on. */}
            {plugin.enabled && plugin.defaultEnabled !== undefined && (
              <div className="plugin-default-row" role="radiogroup" aria-label={`${plugin.name} · ${translate('settings.pluginDefaultLabel')}`} title={translate('settings.pluginDefaultHint')}>
                <span>{translate('settings.pluginDefaultLabel')}</span>
                {[true, false].map((value) => (
                  <button key={String(value)} type="button" role="radio" aria-checked={plugin.defaultEnabled === value}
                    onClick={() => { if (plugin.defaultEnabled !== value) void plugins.setPluginDefaultEnabled(plugin.id, value); }}>
                    {translate(value ? 'settings.pluginDefaultOn' : 'settings.pluginDefaultOff')}
                  </button>
                ))}
              </div>
            )}
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
          {pluginImportState.error && (
            <ErrorMark message={pluginImportState.error} onRetry={() => void handleImportPlugin()} />
          )}
        </div>
        {pluginImportState.message && (
          <p
            className="mt-2 rounded-md bg-[var(--status-success-bg)] px-3 py-2 text-xs text-[var(--status-success-fg)]"
            role="status"
          >
            {pluginImportState.message}
          </p>
        )}
      </div>}
    </>
  );

  const settingsContentNode = (
    <>
      <div
        className={`min-h-0 flex-1 overflow-y-auto ${embedded ? '!overflow-visible !flex-none p-0' : 'px-4 pb-[max(1.5rem,env(safe-area-inset-bottom))] sm:px-5'}`}
      >
        <div className="divide-y divide-[var(--theme-border)]">
          {section === 'notifications' && !hostScope && <NotificationSettings />}
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
                <span className="flex items-center gap-1">
                  {pluginCountLabel}
                  {plugins.error && !plugins.loading && (
                    <ErrorMark message={plugins.error} onRetry={() => void plugins.refresh()} />
                  )}
                </span>
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
          {section === 'preferences' && !hostScope && !relayModeActive() && (
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
          )}
          {section === 'upstreams' && <UpstreamsSettings />}
          {section === 'resources' && <HarnessResourceSettings />}
          {section === 'device' && !hostScope && <LocalPasscodeSettings />}
          {(section === 'device' || section === 'harnesses') && (
            <RuntimeManagement view={section === 'device' && hostScope ? 'supervisor' : section} />
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
      id: 'notifications',
      label: translate('settings.notificationsTab'),
      description: translate('settings.notificationsDescription'),
      account: true,
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
  ];
  return sections.filter((section) => !scoped || section.id === 'preferences' || section.id === 'device').map((section) => ({
    ...section,
    ...(scoped && section.id === 'device' ? { description: translate('settings.treerDeviceDescription') } : {}),
  })).map((section) => ({
    ...section,
    content: <AppShellSettingsDialog embedded section={section.id} />,
  }));
}
