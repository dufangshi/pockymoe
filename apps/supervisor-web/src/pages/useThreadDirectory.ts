import { useEffect, useMemo, useRef, useState } from 'react';
import type { ThreadDetailDto, ThreadDto } from '@pockymoe/shared';
import type { DescribeThread, ThreadLinkInfo } from '@pockymoe/thread-ui';
import { ApiError, fetchThreadDetail } from '../lib/api';

/**
 * Names this device's threads where the page links them (a task's sender, a
 * sub-thread's parent): titles and workspaces from the page's thread list, and a
 * sub-thread's parent read once when the list has settled without it (a reader
 * who may open this thread only). A parent the device refuses or no longer has
 * is `null`. The function changes only when a title or workspace does, so the
 * transcript does not re-render for every list refresh; labels are looked up
 * when asked.
 */
export function useThreadDirectory({ device, detail, threads, listSettled, workspaceLabel }: {
  device: string;
  detail: ThreadDetailDto | null;
  threads: ThreadDto[];
  /** The page's thread list has loaded (or failed to) on this device. */
  listSettled: boolean;
  workspaceLabel: (workspaceId: string) => string | undefined;
}): DescribeThread {
  const parentId = detail?.thread.parentThreadId ?? null;
  const listed = Boolean(parentId && threads.some(thread => thread.id === parentId));
  const readKey = parentId ? `${device}:${parentId}` : null;
  const [read, setRead] = useState<Record<string, ThreadLinkInfo | null>>({});
  const needsRead = Boolean(parentId && readKey && !listed && listSettled && !(readKey in read));
  useEffect(() => {
    if (!needsRead || !parentId || !readKey) return;
    let alive = true;
    void fetchThreadDetail(parentId, { limit: 1 }).then(
      (value): ThreadLinkInfo => ({ title: value.thread.title, workspaceId: value.workspace.id, workspaceLabel: value.workspace.label }),
      // The device's answer (gone, not shared) is final; a lost connection is not.
      (error: unknown) => (error instanceof ApiError ? null : undefined),
    ).then(info => {
      if (alive && info !== undefined) setRead(previous => ({ ...previous, [readKey]: info }));
    });
    return () => { alive = false; };
  }, [needsRead, parentId, readKey]);

  const labels = useRef(workspaceLabel);
  labels.current = workspaceLabel;
  const current = useRef<{ key: string; describe: DescribeThread } | null>(null);
  const readParent = readKey ? read[readKey] : undefined;
  const openId = detail?.thread.id;
  const openTitle = detail?.thread.title;
  const openWorkspace = detail?.thread.workspaceId;
  const openLabel = detail?.workspace.label;
  return useMemo(() => {
    const entries = new Map<string, { title: string; workspaceId: string }>(
      threads.map(thread => [thread.id, { title: thread.title, workspaceId: thread.workspaceId }]),
    );
    if (openId && openTitle !== undefined && openWorkspace) entries.set(openId, { title: openTitle, workspaceId: openWorkspace });
    const key = JSON.stringify([device, [...entries], parentId, readParent ?? String(readParent), openLabel]);
    if (current.current?.key === key) return current.current.describe;
    const describe: DescribeThread = id => {
      const entry = entries.get(id);
      if (!entry) return id === parentId ? readParent : undefined;
      const label = entry.workspaceId === openWorkspace ? openLabel : labels.current(entry.workspaceId);
      return { ...entry, workspaceLabel: label ?? null };
    };
    current.current = { key, describe };
    return describe;
  }, [device, threads, openId, openTitle, openWorkspace, openLabel, parentId, readParent]);
}
