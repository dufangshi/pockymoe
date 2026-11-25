import { render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { setHostPlatform } from '@pockymoe/thread-ui';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { ComposerShortcutSettings } from './ComposerShortcutSettings';

beforeEach(() => setLocale('en'));
afterEach(() => setHostPlatform(null));

// docs/ui-principles.md §19: the keys are written as this platform has them.
it('names Ctrl on Windows and Linux and says a Mac uses ⌘', () => {
  setHostPlatform('win32');
  render(<ComposerShortcutSettings />);
  expect(screen.getByRole('radio', { name: /^Ctrl\+Enter to send/ })).toBeInTheDocument();
  expect(screen.getByText('Enter inserts a new line. Ctrl+Shift+Enter sends directly as steer.')).toBeInTheDocument();
  expect(screen.getByText('Shift+Enter inserts a new line. Ctrl+Enter sends directly as steer.')).toBeInTheDocument();
  expect(screen.getByText(/On a Mac, ⌘ takes the place of Ctrl\.$/)).toBeInTheDocument();
});

it('names ⌘ on a Mac and says Ctrl works too', () => {
  setHostPlatform('darwin');
  render(<ComposerShortcutSettings />);
  expect(screen.getByRole('radio', { name: /^⌘Enter to send/ })).toBeInTheDocument();
  expect(screen.getByText('Enter inserts a new line. ⇧⌘Enter sends directly as steer.')).toBeInTheDocument();
  expect(screen.getByText('⇧Enter inserts a new line. ⌘Enter sends directly as steer.')).toBeInTheDocument();
  expect(screen.getByText(/Ctrl works in place of ⌘ too\.$/)).toBeInTheDocument();
  expect(screen.queryByText(/Ctrl\+Enter/)).toBeNull();
});
