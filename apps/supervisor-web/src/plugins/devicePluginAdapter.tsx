import type { PluginProviderAdapter, PluginSettingsSnapshot, UpstreamConnectionPickerProps } from '@pockymoe/thread-ui';
import type { ImportPluginInput, UpdatePluginInput } from '../../../../packages/shared/src/index';
import { ConnectionPicker } from '../components/ConnectionPicker';
import {
  deletePlugin,
  fetchPluginRuns,
  fetchPlugins,
  importPlugin,
  loadPluginSettings,
  runPluginAction,
  savePluginSettings,
  threadPluginAdapter,
  updatePlugin,
} from '../lib/api';

/**
 * How a PluginProvider reaches one device's plugins. A string names the device,
 * null is this Supervisor; undefined leaves each request to follow the page.
 */
export function devicePluginAdapter(deviceId: string | null | undefined): PluginProviderAdapter {
  return {
    fetchPlugins: () => fetchPlugins(deviceId),
    loadPluginSettings: (id: string) => loadPluginSettings(id, deviceId),
    savePluginSettings: (id: string, settings: PluginSettingsSnapshot) => savePluginSettings(id, settings, deviceId),
    importPlugin: (input: ImportPluginInput) => importPlugin(input, deviceId),
    updatePlugin: (id: string, input: UpdatePluginInput) => updatePlugin(id, input, deviceId),
    deletePlugin: (id: string) => deletePlugin(id, deviceId),
    runPluginAction: (id: string, action: string, input?: unknown) => runPluginAction(id, action, input, deviceId),
    fetchPluginRuns: (id: string) => fetchPluginRuns(id, deviceId),
    threads: threadPluginAdapter(deviceId),
    connections: {
      renderPicker: (props: UpstreamConnectionPickerProps) => (
        <ConnectionPicker
          apiRoot={deviceId && deviceId !== 'local' ? `relay/devices/${encodeURIComponent(deviceId)}/api` : 'api'}
          {...props}
        />
      ),
    },
  };
}
