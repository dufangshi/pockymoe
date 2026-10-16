import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { FileAskBar } from './FileAskBar';

beforeEach(() => setLocale('en'));
const field = () => screen.getByRole('textbox', { name: 'Ask the agent' });
const fill = (text: string) => fireEvent.change(field(), { target: { value: text } });

it('sends trimmed text, clears only on success, and confirms delivery', async () => {
  let resolve!: (sent: boolean) => void;
  const onSend = vi.fn(() => new Promise<boolean>(done => { resolve = done; }));
  render(<FileAskBar onSend={onSend} />);
  fill('  Explain this file. \n ');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(onSend).toHaveBeenCalledExactlyOnceWith('Explain this file.');
  expect(field()).toHaveValue('  Explain this file. \n ');
  expect(screen.getByRole('button', { name: 'Sending...' })).toBeDisabled();
  fireEvent.keyDown(field(), { key: 'Enter' });
  expect(onSend).toHaveBeenCalledTimes(1);
  await act(async () => resolve(true));
  expect(field()).toHaveValue('');
  expect(screen.getByRole('status')).toHaveTextContent(/Sent/);
});

it('keeps the text and explains a false result, then permits correction and retry', async () => {
  const onSend = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  render(<FileAskBar onSend={onSend} />);
  fill(' Keep this question ');
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  await screen.findByRole('status');
  expect(field()).toHaveValue(' Keep this question ');
  expect(screen.getByRole('status')).toHaveTextContent(/not sent|couldn.t send|failed/i);
  fill('Try again');
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(field()).toHaveValue(''));
  expect(onSend).toHaveBeenLastCalledWith('Try again');
});

it('handles a rejected send as a failure without losing text', async () => {
  render(<FileAskBar onSend={vi.fn().mockRejectedValue(new Error('offline'))} />);
  fill('Explain');
  fireEvent.keyDown(field(), { key: 'Enter' });
  expect(await screen.findByRole('status')).toHaveTextContent(/not sent|couldn.t send|failed/i);
  expect(field()).toHaveValue('Explain');
});

it('ignores empty and whitespace-only text even for a direct form submission', () => {
  const onSend = vi.fn();
  render(<FileAskBar onSend={onSend} />);
  for (const value of ['', ' \n\t ']) {
    fill(value);
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    fireEvent.submit(field().closest('form')!);
    fireEvent.keyDown(field(), { key: 'Enter' });
  }
  expect(onSend).not.toHaveBeenCalled();
});

it('Enter sends while Shift+Enter and IME Enter keep editing', async () => {
  const onSend = vi.fn().mockResolvedValue(true);
  render(<FileAskBar onSend={onSend} />);
  fill('First line');
  expect(fireEvent.keyDown(field(), { key: 'Enter', shiftKey: true })).toBe(true);
  expect(fireEvent.keyDown(field(), { key: 'Enter', isComposing: true })).toBe(true);
  expect(onSend).not.toHaveBeenCalled();
  // jsdom does not implement native textarea editing; model Shift+Enter's default edit.
  fill('First line\nSecond line');
  expect(fireEvent.keyDown(field(), { key: 'Enter' })).toBe(false);
  await waitFor(() => expect(field()).toHaveValue(''));
  expect(onSend).toHaveBeenCalledExactlyOnceWith('First line\nSecond line');
});

it('disables the field and sending with a reason and preserves the draft until enabled', async () => {
  const onSend = vi.fn().mockResolvedValue(true);
  const { rerender } = render(<FileAskBar onSend={onSend} />);
  fill('Saved question');
  rerender(<FileAskBar onSend={onSend} disabledReason="Device is offline" />);
  expect(field()).toBeDisabled();
  expect(field()).toHaveAttribute('title', 'Device is offline');
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  fireEvent.submit(field().closest('form')!);
  expect(onSend).not.toHaveBeenCalled();
  rerender(<FileAskBar onSend={onSend} />);
  expect(field()).toBeEnabled();
  expect(field()).toHaveValue('Saved question');
  fireEvent.keyDown(field(), { key: 'Enter' });
  await waitFor(() => expect(onSend).toHaveBeenCalledExactlyOnceWith('Saved question'));
});
