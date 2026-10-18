import { render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { PluginProvider, SettingsDialog } from '@pockymoe/thread-ui';
import { appSettingsSections } from './AppShellSettingsDialog';

const scope = vi.hoisted(() => ({ value: null as null | { kind: string } }));
vi.mock('../lib/hostCapabilities', async (original) => ({
  ...await original<typeof import('../lib/hostCapabilities')>(),
  useHostScope: () => scope.value,
}));

beforeEach(() => { setLocale('en', false); scope.value = null; });

function open(scoped: boolean) {
  render(<PluginProvider adapter={{ fetchPlugins: () => [] }} builtinPlugins={[]}>
    <SettingsDialog open onOpenChange={() => {}} sections={appSettingsSections({ scoped })} initialId="preferences" />
  </PluginProvider>);
}

it('keeps every tab for a standalone Supervisor', () => {
  expect(appSettingsSections().map(section => section.id)).toEqual(
    ['preferences', 'harnesses', 'upstreams', 'resources', 'device', 'workspace', 'plugins', 'advanced']);
});

it('offers a Treer Agent UI only this browser\'s own preferences', () => {
  scope.value = { kind: 'treerAgent' };
  expect(appSettingsSections({ scoped: true }).map(section => section.id)).toEqual(['preferences']);
  open(true);
  expect(screen.getByText('Appearance')).toBeVisible();
  expect(screen.getByLabelText('Text size')).toBeVisible();
  expect(screen.queryByText('Model pricing')).toBeNull();
  expect(screen.queryByText('Message shortcuts')).toBeNull();
});
