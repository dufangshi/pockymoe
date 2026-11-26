import { OutsideAgentsSettings } from './OutsideAgents';
import { SourceControlSettings } from './git/pr/SourceControlSettings';
import { prText } from './git/pr/text';
import { NotificationSettings } from './NotificationSettings';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { LanguageSwitcher } from '@pockymoe/thread-ui/i18n';
import { useFontSize } from '../lib/fontSize';
import { RuntimeManagement } from './RuntimeManagement';
import { LocalPasscodeSettings } from './LocalPasscodeSettings';
import { UpstreamsSettings } from './UpstreamsSettings';
import { HarnessResourceSettings } from './HarnessResourceSettings';
import { DokploySettings } from './DokploySettings';
import { environmentText } from '../lib/environmentI18n';
import { ModelPricingSettings } from './ModelPricingSettings';
import { ComposerShortcutSettings } from './ComposerShortcutSettings';
import { TourSettingsEntry } from './onboarding/TourProvider';
import { ThemePresetSettings } from './ThemePresetSettings';
import { useThemeRegistry } from '../lib/themePacks';
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { useHostScope } from '../lib/hostCapabilities';
import { fetchStorageOverview, relayModeActive } from '../lib/api';
import { useSettingsDevice } from '../lib/settingsDevice';
import {
  PhoneHandChoice,
  SettingsPanels,
  SettingsDialog,
  type SettingsSection,
} from '@pockymoe/thread-ui';
import { useAppShellNav } from './AppShellNavContext';
import { themeOptions } from './appShellNavigationModel';
import { PluginsSettings } from './PluginsSettings';
import { DeviceSettingsLink, SettingsDeviceBar } from './SettingsDeviceBar';
import { WorkspaceDefaultsSettings } from './WorkspaceDefaultsSettings';

/** The tabs that administer one device; the others belong to this browser or the account. */
const DEVICE_SECTIONS = new Set(['harnesses', 'upstreams', 'resources', 'environments', 'device', 'workspace', 'plugins']);

export function AppShellSettingsDialog({
  embedded = false,
  section,
  deviceId,
}: {
  embedded?: boolean;
  section?: string;
  /**
   * The device the device-level sections act on, named by an owner that shows
   * the device itself (a device's own page); null is this Supervisor. Without
   * it the sections follow the page, or the device chosen in the bar they show.
   */
  deviceId?: string | null;
} = {}) {
  useI18n();
  const shellNav = useAppShellNav();
  // One device for every device-level section: read, shown and saved alike.
  const target = useSettingsDevice(deviceId);
  const deviceSection = Boolean(section && DEVICE_SECTIONS.has(section));
  // A host scope (Treer Agent UI) keeps only this browser's own preferences.
  const hostScope = useHostScope();
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
  const effectiveTheme = shellNav?.effectiveTheme ?? 'dark';
  const { title: settingsTitle, description: settingsDescription, extraContent: settingsExtra } = useAppSettingsChrome();
  const [fontSize, setFontSize] = useFontSize();
  const autoCollapseCompletedTurns =
    shellNav?.autoCollapseCompletedTurns ?? true;
  if (!settingsVisible) {
    return null;
  }

  const settingsContentNode = (
    <>
      <div
        className={`min-h-0 flex-1 overflow-y-auto ${embedded ? '!overflow-visible !flex-none p-0' : 'px-4 pb-[max(1.5rem,env(safe-area-inset-bottom))] sm:px-5'}`}
      >
        {/* An owner that names the device already shows it; otherwise say which device this is. */}
        {deviceSection && deviceId === undefined ? <SettingsDeviceBar /> : null}
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
          {section === 'plugins' && !target.missing && <PluginsSettings deviceId={target.deviceId} />}
          {section === 'preferences' && (
            <section className="py-5">
              <label className="flex min-h-11 items-center justify-between gap-4">
                <span>
                  <span className="block text-sm font-semibold">
                    {translate("files.showAgentStatusSummaries")}</span>
                  <span className="settings-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">
                    {translate("files.showIntermediateThinkingSummariesWithTheirOwn")}
                    {relayModeActive() ? ` ${translate("settings.savedToAccount")}` : null}</span>
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
                <ModelPricingSettings embedded />
              </div>
            </details>
          )}

          {section === 'workspace' && !target.missing && <WorkspaceDefaultsSettings key={target.deviceId ?? 'local'} deviceId={target.deviceId} />}
          {section === 'upstreams' && !target.missing && <UpstreamsSettings deviceId={target.deviceId} />}
          {section === 'sourceControl' && !target.missing && <SourceControlSettings />}
          {section === 'resources' && !target.missing && <HarnessResourceSettings deviceId={target.deviceId} />}
          {section === 'environments' && !target.missing && <DokploySettings deviceId={target.deviceId} />}
          {section === 'device' && !hostScope && <LocalPasscodeSettings />}
          {section === 'outside-agents' && !hostScope && <OutsideAgentsSettings />}
          {(section === 'device' || section === 'harnesses') && !target.missing && (
            <RuntimeManagement deviceId={target.deviceId} view={section === 'device' && hostScope ? 'supervisor' : section} />
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
    title={settingsTitle}
    description={settingsDescription}
    extraContent={settingsExtra(closeSettings)}
    sections={appSettingsSections()}
    {...(shellNav?.settingsSection ? { initialId: shellNav.settingsSection } : {})}
    contentProps={{ 'data-testid': 'settingsDialog', onCloseAutoFocus: (event) => event.preventDefault() }}
  />;

}

/** The tabs that administer one device and, in Relay mode, live on that device's
 * own page (its Settings tab, issue #115), not in the settings window. */
const DEVICE_PAGE_SECTIONS = ['device', 'harnesses', 'upstreams', 'resources', 'environments', 'workspace'];

/** Plugin settings open from the gear on their row in Plugins, not as top-level tabs.
 * A host scope (Treer Agent UI) gets Preferences and Device, which there holds only
 * hub maintenance (Check/Update/Restart); the other tabs administer the device.
 * In Relay mode the window keeps this browser's and the account's tabs (and the
 * plugins of a chosen device); a device's own settings are on its page. */
export function appSettingsSections({ scoped = false }: { scoped?: boolean } = {}): SettingsSection[] {
  const relay = relayModeActive();
  return settingsSectionList({ scoped }).filter(section => scoped || !relay || !DEVICE_PAGE_SECTIONS.includes(section.id)).map((section) => ({
    ...section,
    content: <AppShellSettingsDialog embedded section={section.id} />,
  }));
}

/** The Settings tab of a device's page: the device's own settings, then the
 * device-level tabs the settings window had, all acting on `deviceId`. */
export function deviceSettingsSections(deviceId: string, general: ReactNode): SettingsSection[] {
  const sections = settingsSectionList({ scoped: false });
  const byId = (id: string) => sections.find(section => section.id === id)!;
  return [
    { id: 'general', label: translate('devices.settingsGeneral'), description: translate('devices.settingsGeneralDescription'), content: general },
    // "Supervisor" here: on a device's page the device is already named.
    { ...byId('device'), label: 'Supervisor', content: <AppShellSettingsDialog embedded section="device" deviceId={deviceId} /> },
    ...['harnesses', 'upstreams', 'resources', 'environments', 'workspace'].map(id => ({ ...byId(id), content: <AppShellSettingsDialog embedded section={id} deviceId={deviceId} /> })),
  ];
}

function settingsSectionList({ scoped }: { scoped: boolean }): Omit<SettingsSection, 'content'>[] {
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
      id: 'sourceControl',
      label: prText('sourceControl'),
      description: prText('description'),
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
      id: 'environments',
      label: environmentText('title'),
      description: environmentText('description'),
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
      id: 'outside-agents',
      label: environmentText('outsideAgents'),
      description: environmentText('outsideAgentsDescription'),
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
  }));
}

// This Supervisor's name in local mode, read once for the window's title.
let localName: string | null = null;
let localNameRequest: Promise<string | null> | null = null;

/**
 * What the settings window shows around its tabs, for the app shell's window and
 * the thread page's: in Relay mode one line to the device's own settings page;
 * in local mode the title names this machine (issue #115).
 */
export function useAppSettingsChrome(): { title: string | undefined; description: string | undefined; extraContent: (close: () => void) => ReactNode } {
  useI18n();
  const relay = relayModeActive();
  const hostScope = useHostScope();
  const [name, setName] = useState(localName);
  useEffect(() => {
    if (relay || hostScope || localName) return;
    let alive = true;
    localNameRequest ??= fetchStorageOverview().then(overview => overview.deviceName || null).catch(() => { localNameRequest = null; return null; });
    void localNameRequest.then(value => { localName = value; if (alive) setName(value); });
    return () => { alive = false; };
  }, [relay, hostScope]);
  return {
    title: !relay && !hostScope && name ? `${translate('files.settings')} · ${name}` : undefined,
    // In Relay mode the window holds this browser's and the account's settings only.
    description: relay && !hostScope ? translate('settings.windowDescriptionRelay') : undefined,
    extraContent: close => relay && !hostScope ? <DeviceSettingsLink onNavigate={close} /> : null,
  };
}
