import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useMemo, useState, type Dispatch, type SetStateAction } from 'react';

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
import { pathInsideWorkspace } from '../lib/shareRules';
import { environmentResourceRevision } from '../lib/environmentApi';

interface UseThreadWorkspaceAdapterInput {
  setError: Dispatch<SetStateAction<string | null>>;
  workspaceId: string | null;
  threadId?: string | undefined;
  deviceId?: string | null;
  allowLinkedFiles?: boolean;
  access?: 'none' | 'read' | 'write';
  /**
   * A thread or workspace share: the tree starts at the workspace root and no
   * request names a path outside it (`shareRules.filesConfinedToWorkspace`).
   */
  confined?: boolean;
}

export function useThreadWorkspaceAdapter({
  setError,
  workspaceId,
  threadId,
  deviceId,
  access = 'write',
  allowLinkedFiles = false,
  confined = false,
}: UseThreadWorkspaceAdapterInput): ThreadWorkspaceAdapter | null {
  const { locale: i18nLocale } = useI18n();
  const resourceScopeKey = workspaceResourceScope(deviceId);
  const [environmentRevision, setEnvironmentRevision] = useState(() => threadId ? environmentResourceRevision(threadId, deviceId) : 0);
  useEffect(() => {
    setEnvironmentRevision(threadId ? environmentResourceRevision(threadId, deviceId) : 0);
    const changed = (event: Event) => {
      const detail = (event as CustomEvent<{ threadId: string; deviceId?: string | null }>).detail;
      if (detail?.threadId === threadId && (detail.deviceId ?? 'local') === (deviceId ?? 'local')) setEnvironmentRevision(environmentResourceRevision(threadId, deviceId));
    };
    window.addEventListener('pockymoe:environment-changed', changed);
    return () => window.removeEventListener('pockymoe:environment-changed', changed);
  }, [threadId, deviceId]);
  return useMemo<ThreadWorkspaceAdapter | null>(() => {
    if (!workspaceId || access === 'none') {
      return null;
    }

    const isLinked = (path: string) => path.startsWith('/') || /^[a-z]:[\\/]/i.test(path);
    // Refused here, before any request; the device refuses it again.
    const inside = (path: string) => {
      if (confined && !pathInsideWorkspace(path)) throw new Error(translate('files.outsideSharedWorkspace'));
    };
    const insideOrRoot = (path: string) => { if (path !== '' && path !== '.') inside(path); };
    const linkedFiles = allowLinkedFiles && !confined;
    return {
      resourceScopeKey: threadId ? JSON.stringify([resourceScopeKey, threadId, environmentRevision]) : resourceScopeKey,
      textRangeRead: false,
      readDocument: async (input) => {
        inside(input.path);
        try { return await fetchWorkspaceDocument(workspaceId, input.path, input.signal, deviceId, threadId ?? input.threadId); }
        catch (error) {
          if (error instanceof ApiError && [404,501].includes(error.statusCode)) {
            return {path:input.path,name:input.path.split('/').pop()??input.path,language:'text',workspaceRevision:'unsupported',fileIdentity:'',contentHash:null,content:null,size:0,encoding:'unknown',bom:false,eol:'lf',readOnlyReason:'safeUnavailable',truncated:false};
          }
          throw error;
        }
      },
      ...(linkedFiles ? {statLinkedFile: (input: {threadId: string; path: string}) => fetchLinkedFile(input.threadId, input.path, deviceId)} : {}),
      listTree: async (input) => {
        insideOrRoot(input.path ?? '');
        return fetchWorkspaceFileTree(workspaceId, { path: input.path ?? '' }, deviceId, threadId ?? input.threadId);
      },
      readFile: async (input) => {
        inside(input.path);
        // Draw.io XML must be complete; older Supervisors cap text previews at
        // 64 KiB and ignore pagination. Reuse the authenticated/encrypted binary
        // download path so this also works immediately with existing devices.
        if (/\.(drawio|dio)$/i.test(input.path)) {
          const { blob } = isLinked(input.path) && linkedFiles
            ? await downloadLinkedFile(input.threadId, input.path, deviceId)
            : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId, undefined, threadId ?? input.threadId);
          if (blob.size > 8 * 1024 * 1024) throw new Error(translate("files.diagramPreviewSupportsFilesUpTo8"));
          return { path: input.path, name: input.path.replace(/\\/g, '/').split('/').pop() ?? input.path,
            content: await blob.text(), language: 'xml', size: blob.size, truncated: false, nextOffset: blob.size };
        }
        return isLinked(input.path) && linkedFiles ? fetchLinkedFilePreview(input.threadId, input, deviceId) : fetchWorkspaceFilePreview(workspaceId, {
          path: input.path,
          ...(input.offset !== undefined ? { offset: input.offset } : {}),
          ...(input.limit !== undefined ? { limit: input.limit } : {}),
        }, deviceId, threadId ?? input.threadId);
      },
      getRawFileUrl: (input) =>
        // A URL cannot fail in place: an outside path gets an empty one.
        confined && !pathInsideWorkspace(input.path) ? 'data:,'
          : isLinked(input.path) && linkedFiles ? buildLinkedFileUrl(input.threadId, input.path, deviceId) : buildWorkspaceRawFileUrl(workspaceId, { path: input.path }, deviceId, threadId ?? input.threadId),
      // Frames and new tabs pointing at a relay URL bypass the Web origin's
      // encrypted transport; previews such as PDFs load their bytes here.
      readRawFile: async (input) => {
        inside(input.path);
        if (isLinked(input.path) && !linkedFiles) throw new Error(translate("files.linkedFileAccessIsUnavailable"));
        const result = isLinked(input.path)
          ? await downloadLinkedFile(input.threadId, input.path, deviceId, input.signal)
          : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId, input.signal, threadId ?? input.threadId);
        return result.blob;
      },
      ...(access === 'write'
        ? {
            createFile: async (input) => {
              inside(input.path);
              try { return await createWorkspaceFile(workspaceId, input.path, deviceId, threadId ?? input.threadId); }
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
              uploadWorkspaceFile(workspaceId, { file: input.file }, deviceId, threadId ?? input.threadId),
            renameNode: async (input) => { inside(input.fromPath); inside(input.toPath); await renameWorkspaceNode(workspaceId, input, deviceId, threadId ?? input.threadId); },
            deleteNode: async (input) => { inside(input.path); await deleteWorkspaceNode(workspaceId, input.path, deviceId, threadId ?? input.threadId); },
            saveDocument: (input) => {
              inside(input.path);
              if (isLinked(input.path)) throw new Error(translate("files.linkedFilesAreReadOnlyPreviews"));
              const {threadId: _threadId, workspaceId: _workspaceId, ...save} = input;
              return saveWorkspaceDocument(workspaceId,save,deviceId,threadId ?? input.threadId);
            },
            getSaveOperation: (input) => fetchWorkspaceSaveOperation(workspaceId,input.operationId,deviceId,threadId ?? input.threadId),
          }
        : {}),
      // A control that shows the download's progress also shows its failure: the
      // promise rejects. Without one, the failure goes to the workbench's message.
      downloadNode: async (input) => {
        if (!input.onProgress) setError(null);
        try {
          inside(input.path);
          if (isLinked(input.path) && !linkedFiles) throw new Error(translate("files.linkedFileAccessIsUnavailable"));
          const result = isLinked(input.path)
            ? await downloadLinkedFile(input.threadId, input.path, deviceId, undefined, input.onProgress)
            : await downloadWorkspaceFile(workspaceId, { path: input.path }, deviceId, undefined, threadId ?? input.threadId, input.onProgress);
          if (window.pockymoeHost && await (await import('../desktop/download')).nativeSave(result.blob, result.filename)) return;
          const url = URL.createObjectURL(result.blob);
          const anchor = document.createElement('a');
          anchor.href = url;
          anchor.download = result.filename;
          document.body.append(anchor);
          anchor.click();
          anchor.remove();
          // The browser (or the desktop client's download manager) reads a large
          // file from this URL after the click; release it only later.
          window.setTimeout(() => URL.revokeObjectURL(url), 120_000);
        } catch (caught) {
          const message = caught instanceof ApiError
            ? caught.payload.message
            : caught instanceof Error && caught.message
              ? caught.message
              : translate("files.workspaceDownloadFailed");
          if (input.onProgress) throw new Error(message);
          setError(message);
        }
      },
    };
  }, [access, allowLinkedFiles, confined, deviceId, setError, workspaceId, threadId, environmentRevision, i18nLocale, resourceScopeKey]);
}
