import type { PromptAttachmentUpload } from '@pockymoe/thread-ui';
import { downloadLinkedFile } from '../../lib/api';
export interface RewindPrompt { text: string; attachments: { path: string; originalName: string; kind: 'photo' | 'file'; placeholder?: string; clientId?: string }[] }
/** Persisted prompts name durable paths; composer uploads need nonempty local tokens. */
export async function rewindPromptDraft(threadId: string, prompt: RewindPrompt, deviceId?: string | null) {
  let text = prompt.text;
  const attachments: PromptAttachmentUpload[] = [];
  for (const [index, attachment] of prompt.attachments.entries()) {
    const { blob } = await downloadLinkedFile(threadId, attachment.path, deviceId);
    const token = attachment.kind === 'photo' ? 'PHOTO' : 'FILE';
    const placeholder = attachment.placeholder || `[${token} ${attachment.originalName} (${index + 1})]`;
    const persisted = `[${token} ${attachment.path}]`;
    text = text.includes(persisted) ? text.replaceAll(persisted, placeholder) : text.includes(placeholder) ? text : `${text}\n${placeholder}`;
    attachments.push({ ...attachment, clientId: attachment.clientId || crypto.randomUUID(), placeholder, file: new File([blob], attachment.originalName, { type: blob.type }) });
  }
  return { text, attachments };
}
