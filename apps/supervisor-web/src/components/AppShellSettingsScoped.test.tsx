import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
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

function open(scoped: boolean, initialId = 'preferences') {
  render(<MemoryRouter><PluginProvider adapter={{ fetchPlugins: () => [] }} builtinPlugins={[]}>
    <SettingsDialog open onOpenChange={() => {}} sections={appSettingsSections({ scoped })} initialId={initialId} />
  </PluginProvider></MemoryRouter>);
}

it('keeps every tab for a standalone Supervisor', () => {
  expect(appSettingsSections().map(section => section.id)).toEqual(
    ['preferences', 'notifications', 'harnesses', 'upstreams', 'resources', 'device', 'workspace', 'plugins', 'advanced']);
});

it('marks only fully account-scoped tabs', () => {
  expect(appSettingsSections().filter(s => s.account).map(s => s.id)).toEqual(['notifications']);
});

it('offers a Treer Agent UI only this browser\'s own preferences and hub maintenance', () => {
  scope.value = { kind: 'treerAgent' };
  expect(appSettingsSections({ scoped: true }).map(section => section.id)).toEqual(['preferences', 'device']);
  open(true);
  expect(screen.getByText('Appearance')).toBeVisible();
  expect(screen.getByLabelText('Text size')).toBeVisible();
  expect(screen.queryByText('Model pricing')).toBeNull();
  expect(screen.queryByText('Message shortcuts')).toBeNull();
});

it('keeps only Check, Update and Restart on a Treer hub\'s Device tab', async () => {
  scope.value = { kind: 'treerAgent' };
  const requested: string[] = [];
  vi.stubGlobal('fetch', vi.fn(async (url: string | URL) => {
    requested.push(String(url));
    return new Response(JSON.stringify({ runningVersion: '0.13.26', latestVersion: '0.13.27', installedVersion: '0.13.26', canUpdate: true, canRestart: true }),
      { headers: { 'content-type': 'application/json' } });
  }));
  try {
    open(true, 'device');
    expect(await screen.findByText(/0\.13\.26/)).toBeVisible();
    expect(screen.getByRole('button', { name: /Restart Supervisor/ })).toBeVisible();
    expect(screen.getByRole('button', { name: /Check updates/ })).toBeVisible();
    expect(screen.getByRole('button', { name: /^Update$/ })).toBeVisible();
    expect(requested.length).toBeGreaterThan(0);
    expect(requested.every(url => url.endsWith('api/management/supervisor'))).toBe(true);
  } finally {
    vi.unstubAllGlobals();
  }
});
