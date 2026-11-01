import { useMemo, type Dispatch, type SetStateAction } from 'react';

import type { ThreadWorkspaceAdapter } from '@remote-codex/thread-ui';
import {
  ApiError,
  fetchLinkedFile,
  fetchLinkedFilePreview,
  buildLinkedFileUrl,
  buildWorkspaceRawFileUrl,
  downloadWorkspaceFile,
  downloadLinkedFile,
  fetchWorkspaceFilePreview,
  fetchWorkspaceFileTree,
  uploadWorkspaceFile,
  writeWorkspaceFile,
  renameWorkspaceNode,
  deleteWorkspaceNode,
} from '../lib/api';

interface UseThreadWorkspaceAdapterInput {
  setError: Dispatch<SetStateAction<string | null>>;
  workspaceId: string | null;
  allowLinkedFiles?: boolean;
  access?: 'none' | 'read' | 'write';
}

export function useThreadWorkspaceAdapter({
  setError,
  workspaceId,
  access = 'write',
  allowLinkedFiles = false,
}: UseThreadWorkspaceAdapterInput): ThreadWorkspaceAdapter | null {
  return useMemo<ThreadWorkspaceAdapter | null>(() => {
    if (!workspaceId || access === 'none') {
      return null;
    }

    const isLinked = (path: string) => path.startsWith('/') || /^[a-z]:[\\/]/i.test(path);
    return {
      ...(allowLinkedFiles ? {statLinkedFile: (input: {threadId: string; path: string}) => fetchLinkedFile(input.threadId, input.path)} : {}),
      listTree: (input) =>
        fetchWorkspaceFileTree(workspaceId, { path: input.path ?? '' }),
      readFile: (input) =>
        isLinked(input.path) && allowLinkedFiles ? fetchLinkedFilePreview(input.threadId, input) : fetchWorkspaceFilePreview(workspaceId, {
          path: input.path,
          ...(input.offset !== undefined ? { offset: input.offset } : {}),
          ...(input.limit !== undefined ? { limit: input.limit } : {}),
        }),
      getRawFileUrl: (input) =>
        isLinked(input.path) && allowLinkedFiles ? buildLinkedFileUrl(input.threadId, input.path) : buildWorkspaceRawFileUrl(workspaceId, { path: input.path }),
      ...(access === 'write'
        ? {
            uploadFile: (input) =>
              uploadWorkspaceFile(workspaceId, { file: input.file }),
            renameNode: async (input) => { await renameWorkspaceNode(workspaceId, input); },
            deleteNode: async (input) => { await deleteWorkspaceNode(workspaceId, input.path); },
            writeFile: async (input) => {
              if (isLinked(input.path)) throw new Error('Linked files are read-only previews.');
              await writeWorkspaceFile(workspaceId, {
                path: input.path,
                content: input.content,
              });
            },
          }
        : {}),
      downloadNode: async (input) => {
        setError(null);
        try {
          if (isLinked(input.path) && !allowLinkedFiles) throw new Error('Linked file access is unavailable.');
          const result = isLinked(input.path)
            ? await downloadLinkedFile(input.threadId, input.path)
            : await downloadWorkspaceFile(workspaceId, { path: input.path });
          if (window.remoteCodexHost && await (await import('../desktop/download')).nativeSave(result.blob, result.filename)) return;
          const url = URL.createObjectURL(result.blob);
          const anchor = document.createElement('a');
          anchor.href = url;
          anchor.download = result.filename;
          document.body.append(anchor);
          anchor.click();
          anchor.remove();
          URL.revokeObjectURL(url);
        } catch (caught) {
          setError(
            caught instanceof ApiError
              ? caught.payload.message
              : caught instanceof Error
                ? caught.message
                : 'Workspace download failed.',
          );
        }
      },
    };
  }, [access, allowLinkedFiles, setError, workspaceId]);
}
