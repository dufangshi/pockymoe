import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useMemo, type Dispatch, type SetStateAction } from 'react';

import type { ThreadWorkspaceAdapter } from '@pockymoe/thread-ui';
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
  fetchWorkspaceDocument,
  saveWorkspaceDocument,
  fetchWorkspaceSaveOperation,
  workspaceResourceScope,
  renameWorkspaceNode,
  deleteWorkspaceNode,
  createWorkspaceFile,
} from '../lib/api';

interface UseThreadWorkspaceAdapterInput {
  setError: Dispatch<SetStateAction<string | null>>;
  workspaceId: string | null;
  deviceId?: string | null;
  allowLinkedFiles?: boolean;
  access?: 'none' | 'read' | 'write';
}

export function useThreadWorkspaceAdapter({
  setError,
  workspaceId,
  deviceId,
  access = 'write',
  allowLinkedFiles = false,
}: UseThreadWorkspaceAdapterInput): ThreadWorkspaceAdapter | null {
  const { locale: i18nLocale } = useI18n();
  const resourceScopeKey = workspaceResourceScope(deviceId);
  return useMemo<ThreadWorkspaceAdapter | null>(() => {
    if (!workspaceId || access === 'none') {
      return null;
    }

    const isLinked = (path: string) => path.startsWith('/') || /^[a-z]:[\\/]/i.test(path);
    return {
      resourceScopeKey,
      textRangeRead: false,
      readDocument: async (input) => {
        try { return await fetchWorkspaceDocument(workspaceId, input.path, input.signal, deviceId); }
        catch (error) {
          if (error instanceof ApiError && [404,501].includes(error.statusCode)) {
            return {path:input.path,name:input.path.split('/').pop()??input.path,language:'text',workspaceRevision:'unsupported',fileIdentity:'',contentHash:null,content:null,size:0,encoding:'unknown',bom:false,eol:'lf',readOnlyReason:'safeUnavailable',truncated:false};
          }
          throw error;
        }
      },
      ...(allowLinkedFiles ? {statLinkedFile: (input: {threadId: string; path: string}) => fetchLinkedFile(input.threadId, input.path, deviceId)} : {}),
      listTree: (input) =>
        fetchWorkspaceFileTree(workspaceId, { path: input.path ?? '' }, deviceId),
      readFile: async (input) => {
        // Draw.io XML must be complete; older Supervisors cap text previews at
        // 64 KiB and ignore pagination. Reuse the authenticated/encrypted binary
        // download path so this also works immediately with existing devices.
        if (/\.(drawio|dio)$/i.test(input.path)) {
          const { blob } = isLinked(input.path) && allowLinkedFiles
            ? await downloadLinkedFile(input.threadId, input.path, deviceId)
            : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId);
          if (blob.size > 8 * 1024 * 1024) throw new Error(translate("files.diagramPreviewSupportsFilesUpTo8"));
          return { path: input.path, name: input.path.replace(/\\/g, '/').split('/').pop() ?? input.path,
            content: await blob.text(), language: 'xml', size: blob.size, truncated: false, nextOffset: blob.size };
        }
        return isLinked(input.path) && allowLinkedFiles ? fetchLinkedFilePreview(input.threadId, input, deviceId) : fetchWorkspaceFilePreview(workspaceId, {
          path: input.path,
          ...(input.offset !== undefined ? { offset: input.offset } : {}),
          ...(input.limit !== undefined ? { limit: input.limit } : {}),
        }, deviceId);
      },
      getRawFileUrl: (input) =>
        isLinked(input.path) && allowLinkedFiles ? buildLinkedFileUrl(input.threadId, input.path, deviceId) : buildWorkspaceRawFileUrl(workspaceId, { path: input.path }, deviceId),
      // Frames and new tabs pointing at a relay URL bypass the Web origin's
      // encrypted transport; previews such as PDFs load their bytes here.
      readRawFile: async (input) => {
        if (isLinked(input.path) && !allowLinkedFiles) throw new Error(translate("files.linkedFileAccessIsUnavailable"));
        const result = isLinked(input.path)
          ? await downloadLinkedFile(input.threadId, input.path, deviceId, input.signal)
          : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId, input.signal);
        return result.blob;
      },
      ...(access === 'write'
        ? {
            createFile: async (input) => {
              try { return await createWorkspaceFile(workspaceId, input.path, deviceId); }
              catch (error) {
                if (error instanceof ApiError) {
                  const code = error.payload.code;
                  if (code === 'fileAlreadyExists') throw new Error(translate('files.fileAlreadyExists'));
                  if (code === 'permissionDenied') throw new Error(translate('files.createPermissionDenied'));
                  if ([404,405,501].includes(error.statusCode)) throw new Error(translate('files.createUnavailable'));
                }
                throw error;
              }
            },
            uploadFile: (input) =>
              uploadWorkspaceFile(workspaceId, { file: input.file }, deviceId),
            renameNode: async (input) => { await renameWorkspaceNode(workspaceId, input, deviceId); },
            deleteNode: async (input) => { await deleteWorkspaceNode(workspaceId, input.path, deviceId); },
            saveDocument: (input) => {
              if (isLinked(input.path)) throw new Error(translate("files.linkedFilesAreReadOnlyPreviews"));
              const {threadId: _threadId, workspaceId: _workspaceId, ...save} = input;
              return saveWorkspaceDocument(workspaceId,save,deviceId);
            },
            getSaveOperation: (input) => fetchWorkspaceSaveOperation(workspaceId,input.operationId,deviceId),
          }
        : {}),
      downloadNode: async (input) => {
        setError(null);
        try {
          if (isLinked(input.path) && !allowLinkedFiles) throw new Error(translate("files.linkedFileAccessIsUnavailable"));
          const result = isLinked(input.path)
            ? await downloadLinkedFile(input.threadId, input.path, deviceId)
            : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId);
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
                : translate("files.workspaceDownloadFailed"),
          );
        }
      },
    };
  }, [access, allowLinkedFiles, deviceId, setError, workspaceId, i18nLocale, resourceScopeKey]);
}
