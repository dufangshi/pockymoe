import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';

import { useAppShellNav } from '../components/AppShellNavContext';

/** Old names of a settings tab in `?tab=` links (the mobile shell's "connections"). */
const TAB_ALIASES: Record<string, string> = { connections: 'upstreams' };

/**
 * `/relay-settings` opens the settings window, at `?tab=` if given. A device's own
 * tab (Harness, upstreams, …) opens its device page's Settings tab instead (issue #115).
 */
export function RelaySettingsPage() {
  useI18n();
  const shellNav = useAppShellNav();
  const [params] = useSearchParams();
  const tab = params.get('tab');

  useEffect(() => {
    if (tab && shellNav?.openSettingsSection) shellNav.openSettingsSection(TAB_ALIASES[tab] ?? tab);
    else shellNav?.openSettings();
  }, [shellNav, tab]);

  return (
    <div className="sr-only" data-testid="relay-settings-page">
      {translate("settings.settings")}</div>
  );
}
