import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import {
  PluginProvider,
  SettingsDialog,
  type FrontendPluginModule,
} from '@pockymoe/thread-ui';
import type { ComponentProps } from 'react';
import type { PluginDto, PluginManifestDto } from '@pockymoe/shared';
import { appSettingsSections } from './AppShellSettingsDialog';

function manifest(id: string, name: string): PluginManifestDto {
  return {
    id,
    name,
    version: '1.0.0',
    description: `${name} description`,
    remoteCodex: '>=0.13.0',
    capabilities: { artifactTypes: [], timelineRenderers: [], threadPanels: [], actions: [] },
  };
}

const configurable: FrontendPluginModule = {
  manifest: manifest('example.configurable', 'Configurable'),
  defaultEnabled: false,
  settingsSections: [
    { id: 'first', label: 'Output', description: 'Output options', render: () => <label>Style<input /></label> },
    { id: 'second', label: 'Limits', render: context => <p>Limits for {context.pluginId}</p> },
  ],
};
const plain: FrontendPluginModule = { manifest: manifest('example.plain', 'Plain') };
const missing: FrontendPluginModule = {
  manifest: manifest('example.missing', 'Missing'),
  settingsSections: [{ id: 'only', label: 'Missing options', render: () => <p>Unavailable form</p> }],
};

function setup() {
  const reported: PluginDto[] = [
    { ...configurable.manifest, enabled: false, available: true },
    { ...plain.manifest, enabled: true, available: true },
    { ...missing.manifest, enabled: false, available: false, unavailableReason: 'Not installed' } as PluginDto,
  ];
  const updatePlugin = vi.fn(async (id: string, input: { enabled?: boolean }) => ({ ...reported.find(plugin => plugin.id === id)!, ...input }));
  const adapter: NonNullable<ComponentProps<typeof PluginProvider>['adapter']> = {
    fetchPlugins: () => reported,
    updatePlugin,
    loadPluginSettings: async () => ({ revision: 1, values: {} }),
    savePluginSettings: async (_id, settings) => settings,
  };
  const onOpenChange = vi.fn();
  render(
    <PluginProvider adapter={adapter} builtinPlugins={[configurable, plain, missing]}>
      <SettingsDialog open onOpenChange={onOpenChange} sections={appSettingsSections()} initialId="plugins" />
    </PluginProvider>,
  );
  return { updatePlugin, onOpenChange };
}

beforeEach(() => setLocale('en', false));

it('opens a plugin settings dialog from a gear without leaving Settings or toggling the plugin', async () => {
  const { updatePlugin, onOpenChange } = setup();
  const settings = screen.getByRole('dialog', { name: 'Settings' });
  // Plugin settings are no longer top-level Settings tabs.
  expect(within(settings).getAllByRole('tab').map(tab => tab.textContent)).not.toContain('Output');
  const gear = await within(settings).findByRole('button', { name: 'Configurable settings' });
  expect(gear).toHaveAttribute('title', 'Configurable settings');
  // Only plugins that contribute settings and are available on this device get a gear.
  expect(within(settings).queryByRole('button', { name: 'Plain settings' })).not.toBeInTheDocument();
  expect(within(settings).queryByRole('button', { name: 'Missing settings' })).not.toBeInTheDocument();
  const toggle = within(settings).getByRole('checkbox', { name: 'Configurable enabled' });
  expect(toggle).not.toBeChecked();
  expect(gear.closest('label')).toBeNull();

  fireEvent.click(gear);
  const child = await screen.findByRole('dialog', { name: 'Configurable settings' });
  expect(toggle).not.toBeChecked();
  expect(updatePlugin).not.toHaveBeenCalled();
  // Disabled plugins stay configurable, and every contributed section is shown.
  expect(within(child).getByRole('heading', { name: 'Output' })).toBeVisible();
  expect(within(child).getByRole('heading', { name: 'Limits' })).toBeVisible();
  expect(within(child).getByLabelText('Style')).toBeVisible();
  expect(within(child).getByText('Limits for example.configurable')).toBeVisible();

  fireEvent.keyDown(within(child).getByLabelText('Style'), { key: 'Escape' });
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Configurable settings' })).not.toBeInTheDocument());
  expect(screen.getByRole('dialog', { name: 'Settings' })).toBeVisible();
  expect(onOpenChange).not.toHaveBeenCalled();
  await waitFor(() => expect(gear).toHaveFocus());

  // The plugin name still toggles its checkbox.
  fireEvent.click(within(settings).getByText('Configurable', { exact: true }));
  await waitFor(() => expect(updatePlugin).toHaveBeenCalledWith('example.configurable', { enabled: true }));
});

it('localizes the gear and dialog title', async () => {
  setLocale('zh-CN', false);
  setup();
  const gear = await screen.findByRole('button', { name: 'Configurable设置' });
  fireEvent.click(gear);
  expect(await screen.findByRole('dialog', { name: 'Configurable设置' })).toBeVisible();
});
