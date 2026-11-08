import { useCallback, useEffect, useRef, useState } from 'react';
import type { ComposerUploadProgressState, PromptAttachmentUpload } from '@pockymoe/thread-ui';
import { translate as t } from '@pockymoe/thread-ui/i18n';
import { ApiError, deviceRequest } from './api';

/**
 * Attachments too large for one request go to the device in chunks before their
 * prompt, which then only names the finished files. The Relay forwards one chunk
 * at a time and keeps nothing (docs/prompt-attachments.md).
 */
export const MAX_ATTACHMENT_BYTES = 1024 * 1024 * 1024;
/** Up to here a prompt and its files still fit one request, on any device. */
const ONE_REQUEST_BYTES = 16 * 1024 * 1024;
/** What a device without the chunked routes accepts in one request. */
const OLD_DEVICE_FILE_BYTES = 25 * 1024 * 1024;
const OLD_DEVICE_TOTAL_BYTES = 30 * 1024 * 1024;
const MIN_CHUNK_BYTES = 512 * 1024;
const RETRIES = 8;
const GROW_AFTER = 3;

type Progress = (state: ComposerUploadProgressState | null) => void;

/** Finished or partly sent files of a prompt, so sending it again continues them. */
const sent = new Map<string, { thread: string; uploadId: string; token?: string }>();

class Aborted extends Error {}

function retryable(error: unknown) {
  if (!(error instanceof ApiError)) return true;
  return error.statusCode === 429 || error.statusCode === 408 || error.statusCode >= 502 && error.statusCode <= 504
    || String(error.payload.code) === 'transport_reconnect_required';
}

function wait(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener('abort', () => { clearTimeout(timer); reject(new Aborted()); }, { once: true });
  });
}

async function sendFile(
  thread: string,
  attachment: PromptAttachmentUpload,
  device: string | null | undefined,
  signal: AbortSignal,
  report: (loaded: number, retrying?: boolean) => void,
): Promise<string> {
  const { file } = attachment;
  const known = sent.get(attachment.clientId);
  if (known?.token && known.thread === thread) return known.token;
  const base = `api/threads/${thread}/uploads`;
  let uploadId = known?.thread === thread ? known.uploadId : null;
  let received = 0;
  let chunk = 4 * 1024 * 1024;
  if (uploadId) {
    try {
      const status = await deviceRequest<{ received: number; token?: string | null }>(device, `${base}/${uploadId}`, { signal });
      received = status.received;
    } catch (error) {
      if (!(error instanceof ApiError && error.statusCode === 404)) throw error;
      uploadId = null; // It expired on the device: start over.
    }
  }
  if (!uploadId) {
    const begun = await deviceRequest<{ uploadId: string; chunkSize: number }>(device, base, {
      method: 'POST', signal,
      body: JSON.stringify({ name: attachment.originalName.trim() || file.name, size: file.size, kind: attachment.kind }),
    });
    uploadId = begun.uploadId;
    chunk = begun.chunkSize;
    sent.set(attachment.clientId, { thread, uploadId });
  }
  const max = chunk;
  let failures = 0;
  let good = 0;
  while (received < file.size) {
    report(received);
    if (signal.aborted) throw new Aborted();
    const end = Math.min(received + chunk, file.size);
    try {
      const answer = await deviceRequest<{ received: number }>(device, `${base}/${uploadId}?offset=${received}`, {
        method: 'PUT', signal, body: file.slice(received, end), headers: { 'Content-Type': 'application/octet-stream' },
      });
      received = answer.received;
      failures = 0;
      if (++good >= GROW_AFTER) { good = 0; chunk = Math.min(chunk * 2, max); }
    } catch (error) {
      if (signal.aborted) throw new Aborted();
      if (error instanceof ApiError && error.statusCode === 409) {
        // An earlier try landed after all: continue from where the device is.
        received = (await deviceRequest<{ received: number }>(device, `${base}/${uploadId}`, { signal })).received;
        continue;
      }
      if (!retryable(error) || ++failures > RETRIES) throw error;
      // A chunk that keeps timing out on a slow link gets smaller.
      good = 0;
      chunk = Math.max(Math.floor(chunk / 2), MIN_CHUNK_BYTES);
      report(received, true);
      await wait(Math.min(30_000, 1000 * 2 ** (failures - 1)), signal);
    }
  }
  report(file.size);
  for (;;) {
    try {
      const { token } = await deviceRequest<{ token: string }>(device, `${base}/${uploadId}/complete`, { method: 'POST', signal, body: '{}' });
      sent.set(attachment.clientId, { thread, uploadId, token });
      return token;
    } catch (error) {
      if (signal.aborted) throw new Aborted();
      if (!retryable(error) || ++failures > RETRIES) throw error;
      report(file.size, true);
      await wait(Math.min(30_000, 1000 * 2 ** (failures - 1)), signal);
    }
  }
}

/** Sends what is too large for the prompt's own request and rewrites the prompt to name it. */
export async function prepareAttachments<T extends { prompt: string; attachments?: PromptAttachmentUpload[] }>(
  thread: string, input: T, device: string | null | undefined, signal: AbortSignal, onProgress: Progress,
): Promise<T> {
  const attachments = input.attachments ?? [];
  const total = attachments.reduce((sum, attachment) => sum + attachment.file.size, 0);
  if (attachments.some((attachment) => attachment.file.size > MAX_ATTACHMENT_BYTES)) throw new Error(t('workbench.attachmentOver1Gb'));
  if (total <= ONE_REQUEST_BYTES) return input;
  let prompt = input.prompt;
  try {
    for (const [index, attachment] of attachments.entries()) {
      const name = attachment.originalName.trim() || attachment.file.name;
      const token = await sendFile(thread, attachment, device, signal, (loaded, retrying) => onProgress({
        name, loaded, total: attachment.file.size, index: index + 1, count: attachments.length, ...(retrying ? { retrying } : {}),
      }));
      prompt = prompt.split(attachment.placeholder).join(token);
    }
  } catch (error) {
    // A device from before these routes still takes what fits one request.
    if (error instanceof ApiError && (error.statusCode === 404 || error.statusCode === 405) && !sent.has(attachments[0]!.clientId)) {
      if (attachments.every((attachment) => attachment.file.size <= OLD_DEVICE_FILE_BYTES) && total <= OLD_DEVICE_TOTAL_BYTES) return input;
      throw new Error(t('workbench.deviceNeedsUpdateForLargeFiles'));
    }
    throw error;
  }
  for (const attachment of attachments) sent.delete(attachment.clientId);
  const { attachments: _sent, ...rest } = input;
  return { ...rest, prompt } as T;
}

/**
 * The upload of one composer. Leaving the conversation stops it where it is;
 * sending the same draft again continues there.
 */
export function useAttachmentUploads(thread: string | null, device: string | null | undefined) {
  const [progress, setProgress] = useState<ComposerUploadProgressState | null>(null);
  const running = useRef<{ controller: AbortController; ids: string[] } | null>(null);

  useEffect(() => () => { running.current?.controller.abort(); running.current = null; }, [thread, device]);

  /** Null when it was stopped: the draft stays and nothing is sent. */
  const upload = useCallback(async <T extends { prompt: string; attachments?: PromptAttachmentUpload[] }>(input: T): Promise<T | null> => {
    if (!thread) return input;
    const controller = new AbortController();
    running.current = { controller, ids: (input.attachments ?? []).map((attachment) => attachment.clientId) };
    try {
      return await prepareAttachments(thread, input, device, controller.signal, (state) => {
        if (!controller.signal.aborted) setProgress(state);
      });
    } catch (error) {
      if (controller.signal.aborted) return null;
      throw error;
    } finally {
      if (running.current?.controller === controller) running.current = null;
      setProgress(null);
    }
  }, [thread, device]);

  /** The person gave up on it: the partial file goes too. */
  const cancel = useCallback(() => {
    const current = running.current;
    if (!current || !thread) return;
    current.controller.abort();
    for (const id of current.ids) {
      const known = sent.get(id);
      if (known && !known.token) {
        sent.delete(id);
        void deviceRequest(device, `api/threads/${known.thread}/uploads/${known.uploadId}`, { method: 'DELETE' }).catch(() => undefined);
      }
    }
  }, [thread, device]);

  return { progress, upload, cancel };
}
