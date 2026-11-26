import { expect, it, vi } from 'vitest';
import { downloadLinkedFile } from '../../lib/api';
import { rewindPromptDraft } from './rewindPrompt';
vi.mock('../../lib/api', () => ({ downloadLinkedFile: vi.fn() }));
it('turns durable paths and empty placeholders into safe composer upload chips', async () => {
  vi.mocked(downloadLinkedFile).mockResolvedValue({ blob: new Blob(['content']), filename: 'notes.txt' });
  const draft = await rewindPromptDraft('t', { text: 'Read [FILE ./.temp/threads/t/notes.txt]', attachments: [{ path: './.temp/threads/t/notes.txt', originalName: 'notes.txt', kind: 'file', placeholder: '' }] });
  expect(draft.text).toBe('Read [FILE notes.txt (1)]');
  expect(draft.attachments[0]!.placeholder).toBe('[FILE notes.txt (1)]');
  expect(draft.attachments[0]!.clientId).toBeTruthy();
  expect(draft.attachments[0]!.file.size).toBe(7);
});
