import { useMemo, useState, type ReactNode } from 'react';
import { RefreshCw } from 'lucide-react';
import { ErrorMark, PluginProvider, PluginSettingsButton, useDevicePlugins } from '@pockymoe/thread-ui';
import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { useRoutedDeviceId } from '../lib/settingsDevice';
import { builtinFrontendPlugins } from '../plugins/builtin-plugin-modules';
import { devicePluginAdapter } from '../plugins/devicePluginAdapter';
import { iconButton } from './ConnectionCatalog';

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

/** A built-in plugin's name in the reader's language; others keep their manifest name. */
function pluginName(plugin: { id: string; name: string }) {
  return plugin.id === 'remote-codex.terminal' ? translate('files.terminalMenu') : plugin.name;
}

/** What a plugin adds, in words: a terminal panel, a Harness mode; other types keep their id. */
function pluginCapabilityLabel(kind: string) {
  if (kind === 'terminal') return translate('files.terminalMenu');
  if (kind.startsWith('harness:')) return 'Harness';
  return kind;
}

/**
 * The plugins of one device. On a conversation or workspace page that is the
 * routed device, whose plugins the page already holds; anywhere else (the
 * devices pages, the map, account pages) this reads the named device's own,
 * never the plugins of a device remembered from an earlier page (issue #112).
 */
export function PluginsSettings({ deviceId }: { deviceId: string | null }) {
  const routed = useRoutedDeviceId();
  if (!deviceId || deviceId === routed) return <PluginList />;
  return <DevicePlugins key={deviceId} deviceId={deviceId}><PluginList /></DevicePlugins>;
}

function DevicePlugins({ deviceId, children }: { deviceId: string; children: ReactNode }) {
  const adapter = useMemo(() => devicePluginAdapter(deviceId), [deviceId]);
  return <PluginProvider scope={deviceId} adapter={adapter} builtinPlugins={builtinFrontendPlugins}>{children}</PluginProvider>;
}

function PluginList() {
  useI18n();
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
  const enabledPluginCount = plugins.plugins.filter(
    (plugin) => plugin.enabled,
  ).length;
  const pluginCountLabel = plugins.loading
    ? translate("files.loading")
    : translate('files.pluginsEnabledCount', { value1: enabledPluginCount, value2: plugins.plugins.length });
  const anyUnavailable = plugins.plugins.some((plugin) => unavailablePluginReason(plugin) !== null);
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
                  {pluginName(plugin)}
                </span>
                {/* What the plugin does decides whether to turn it on: a note, never hidden by a pack. */}
                <span className="settings-note mt-1 block text-xs leading-5 text-[var(--theme-fg-muted)]">
                  {pluginDescription(plugin)}
                </span>
                <span className="settings-hint mt-1 block text-[11px] leading-5 text-[var(--theme-fg-muted)]">
                  {[
                    [...new Set([
                      ...plugin.capabilities.artifactTypes.map((type) => type.type),
                      ...plugin.capabilities.threadPanels.map(
                        (panel) => panel.kind ?? panel.id,
                      ),
                    ].map(pluginCapabilityLabel))].join(', ') || translate("files.utility"),
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
                  aria-label={translate("files.enabled", { value1: pluginName(plugin) })}
                  onChange={(event) =>
                    void plugins.setPluginEnabled(
                      plugin.id,
                      event.currentTarget.checked,
                    )
                  }
                  type="checkbox"
                />
              </span>
              {/* A plugin this device cannot run keeps its switch, greyed, with the reason on a mark.
                  Every row keeps the mark's slot while any has one, so the switches stay in one column. */}
              {anyUnavailable ? (
                <span className="grid h-8 w-8 shrink-0 place-items-center">
                  {unavailable ? <ErrorMark kind="unavailable" message={unavailable} /> : null}
                </span>
              ) : null}
            </div>
            {/* An enabled plugin says whether conversations that follow the default have it on. */}
            {plugin.enabled && plugin.defaultEnabled !== undefined && (
              <div className="plugin-default-row" title={translate('settings.pluginDefaultHint')}>
                <span>{translate('settings.pluginDefaultLabel')}</span>
                <span className="product-segmented is-compact" role="radiogroup" aria-label={`${pluginName(plugin)} · ${translate('settings.pluginDefaultLabel')}`}>
                  {[true, false].map((value) => (
                    <button key={String(value)} type="button" role="radio" className="product-segment" aria-checked={plugin.defaultEnabled === value}
                      onClick={() => { if (plugin.defaultEnabled !== value) void plugins.setPluginDefaultEnabled(plugin.id, value); }}>
                      {translate(value ? 'settings.pluginDefaultOn' : 'settings.pluginDefaultOff')}
                    </button>
                  ))}
                </span>
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

  return (
    <section className="py-2">
      <div className="flex items-center justify-between text-xs text-[var(--theme-fg-muted)]">
        <span className="flex items-center gap-1">
          {pluginCountLabel}
          {plugins.error && !plugins.loading && (
            <ErrorMark message={plugins.error} onRetry={() => void plugins.refresh()} />
          )}
        </span>
        {/* The same bordered refresh key as Upstreams and Isolated environments. */}
        <button
          type="button"
          className={iconButton}
          aria-label={translate("files.refreshPlugins")}
          title={translate("files.refreshPlugins")}
          disabled={plugins.loading}
          onClick={() => void plugins.refresh()}
        >
          <RefreshCw size={14} aria-hidden className={plugins.loading ? 'request-spin' : undefined} /></button>
      </div>
      {pluginsManagementNode}
    </section>
  );
}
