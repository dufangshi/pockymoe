import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { setLocale } from '@pockymoe/thread-ui/i18n';
import { gitWrite } from '../../lib/gitApi';
import { RewindDialog } from './RewindDialog';
vi.mock('../../lib/gitApi', () => ({ gitWrite: vi.fn() }));
it('disables file rewind with the restore refusal, and returns the conversation prompt', async () => {
  setLocale('en');
  vi.mocked(gitWrite).mockRejectedValueOnce(new Error('Working copy used by another thread')).mockResolvedValueOnce({ prompt: { text: 'Fix the parser', attachments: [] } });
  const restored = vi.fn(), close = vi.fn();
  render(<RewindDialog threadId="t" turnId="one" onClose={close} onRestored={restored} />);
  await screen.findByText('Error: Working copy used by another thread');
  expect(screen.getByRole('radio', { name: 'Rewind conversation and files' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Rewind', exact: true }));
  await waitFor(() => expect(restored).toHaveBeenCalledWith({ text: 'Fix the parser', attachments: [] }));
  expect(gitWrite).toHaveBeenLastCalledWith('t', 'rewind', { turnId: 'one', restoreFiles: false }, undefined);
  expect(close).toHaveBeenCalledOnce();
});
