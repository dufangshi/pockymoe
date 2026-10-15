import { render, screen } from '@testing-library/react';
import { setLocale, translate } from '@pockymoe/thread-ui/i18n';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { beforeEach, expect, it } from 'vitest';
import { protectionView } from '../components/DeviceProtectionStatus';
import type { ProtectionProgress } from '../lib/remoteDeviceProtection';
import { HELP_TOPICS, HelpPage, headingAnchor, helpSource } from './HelpPage';

beforeEach(() => setLocale('en'));
const anchors = (source: string) => [...source.matchAll(/\{#([a-z0-9-]+)\}/g)].map(m => m[1]);

it.each(['en', 'zh-CN'])('has every topic in %s with the same anchors as English', locale => {
  for (const topic of HELP_TOPICS) {
    const source = helpSource(locale, topic.id)!;
    expect(source, topic.id).toMatch(/^# /);
    expect(anchors(source)).toEqual(anchors(helpSource('en', topic.id)!));
  }
});

it('has an answer for every device status the Devices page links to', () => {
  const phases: ProtectionProgress[] = [
    { phase: 'queued' }, { phase: 'update-required' }, { phase: 'updating' }, { phase: 'binding' },
    { phase: 'certifying' }, { phase: 'identity-changed' }, { phase: 'certificate-missing' },
    { phase: 'local-command-pending' }, { phase: 'error', error: 'account-mismatch' },
    { phase: 'error', error: 'update-failed' }, { phase: 'error', error: 'failed' },
  ];
  const linked = new Set([
    ...phases.map(p => protectionView(p, 'unknown')!.anchor),
    ...(['protected', 'not-protected', 'pending', 'certificate-missing'] as const).map(s => protectionView(undefined, s)!.anchor),
  ]);
  for (const locale of ['en', 'zh-CN']) {
    const available = new Set(anchors(helpSource(locale, 'devices')!));
    for (const anchor of linked) expect(available.has(anchor), `${locale} #${anchor}`).toBe(true);
  }
});

it('reads heading anchors', () => {
  expect(headingAnchor('Bound to another account {#other-account}')).toEqual({ title: 'Bound to another account', id: 'other-account' });
  expect(headingAnchor('Plain heading')).toEqual({ title: undefined, id: undefined });
});

it.each(['en', 'zh-CN'] as const)('renders a topic in %s with linkable answers', locale => {
  setLocale(locale);
  render(<MemoryRouter initialEntries={['/help/devices#other-account']}><Routes><Route element={<HelpPage />} path="/help/:topic" /></Routes></MemoryRouter>);
  const heading = screen.getByRole('heading', { level: 2, name: locale === 'en' ? 'Bound to another account' : '绑定到其他账户' });
  expect(heading).toHaveAttribute('id', 'other-account');
  expect(screen.getByRole('link', { name: translate('common.helpDevices') })).toHaveAttribute('aria-current', 'page');
});

it('lists topics on the index and reports unknown topics', () => {
  const { unmount } = render(<MemoryRouter initialEntries={['/help']}><Routes><Route element={<HelpPage />} path="/help" /></Routes></MemoryRouter>);
  expect(screen.getAllByRole('link', { name: /Devices and protection/ })).toHaveLength(2);
  unmount();
  render(<MemoryRouter initialEntries={['/help/missing']}><Routes><Route element={<HelpPage />} path="/help/:topic" /></Routes></MemoryRouter>);
  expect(screen.getByRole('alert')).toHaveTextContent(translate('common.helpNotFound'));
});
