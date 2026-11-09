import { useCallback, useEffect, useState } from 'react';
import { Box, GitBranch } from 'lucide-react';
import type { WorkbenchInspector, WorkbenchToolPanel } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import type { ThreadDto } from '@pockymoe/shared';
import { environmentText as et } from '../lib/environmentI18n';
import { ThreadGitPanel } from './git/ThreadGitPanel';
import { ThreadIsolationPanel, isolationMenuLabel, useIsolationAttention } from './environments/ThreadIsolationPanel';
import { requestMark } from './environments/ui';
import './thread-inspector.css';

export type InspectorView = 'git' | 'isolation';
// Only which inspector was open is remembered (per browser), never its content.
const STORAGE_KEY = 'pockymoe.workbench.inspector';
const readView = (): InspectorView | null => {
  try { const value = localStorage.getItem(STORAGE_KEY); return value === 'git' || value === 'isolation' ? value : null; } catch { return null; }
};

/**
 * The workbench's right dock for Git and isolation. Left stays navigation (rail
 * and thread list), the center the conversation; this dock inspects the open
 * thread beside it without covering it. The rail toggles each view, Files shares
 * the dock (opening one closes the other) and on a phone the dock is a full
 * view that Back closes (docs/ui-principles.md §4).
 */
export function useThreadInspector({ thread, deviceId, canManage, gitAvailable, isolationAvailable, filesOpen, closeFiles }: {
  thread: Pick<ThreadDto, 'id' | 'title' | 'status' | 'agentId' | 'provider' | 'providerSessionId' | 'updatedAt' | 'parentThreadId'> | null | undefined;
  deviceId: string | null | undefined;
  canManage: boolean;
  gitAvailable: boolean;
  isolationAvailable: boolean;
  filesOpen: boolean;
  /** False when closing Files was refused (unsaved documents in another workspace). */
  closeFiles(): boolean | void;
}): { toolPanels: WorkbenchToolPanel[]; inspector: WorkbenchInspector | null } {
  useI18n();
  const [view, setViewState] = useState<InspectorView | null>(readView);
  const setView = useCallback((next: InspectorView | null) => {
    setViewState(next);
    try { if (next) localStorage.setItem(STORAGE_KEY, next); else localStorage.removeItem(STORAGE_KEY); } catch { /* Optional preference. */ }
  }, []);
  // Files and the inspectors share the dock: Files opening closes an inspector,
  // and forgets it, so a reload does not bring back the view Files replaced.
  useEffect(() => { if (filesOpen) setView(null); }, [filesOpen, setView]);
  const shown = view === 'git' && gitAvailable ? 'git' : view === 'isolation' && isolationAvailable ? 'isolation' : null;
  const attention = useIsolationAttention(isolationAvailable ? thread?.id : undefined, thread?.status, deviceId, shown === 'isolation');
  const toggle = (next: InspectorView) => {
    if (shown === next) { setView(null); return; }
    // Files stays open when the reader keeps unsaved documents: then nothing switches.
    if (filesOpen && closeFiles() === false) return;
    setView(next);
  };
  const isolationLabel = isolationMenuLabel(attention);
  const toolPanels: WorkbenchToolPanel[] = [
    ...(gitAvailable ? [{ id: 'git', label: 'Git', icon: <GitBranch data-icon="git" />, active: shown === 'git', onToggle: () => toggle('git') }] : []),
    ...(isolationAvailable ? [{ id: 'isolation', label: isolationLabel, active: shown === 'isolation', onToggle: () => toggle('isolation'),
      attention: attention ? requestMark(attention.state) : undefined,
      icon: <span className="inspector-rail-icon" data-attention={attention ? requestMark(attention.state) : undefined}><Box data-icon="isolation" /></span> }] : []),
  ];
  const scope = `${deviceId ?? 'local'}:${thread?.id ?? ''}`;
  const inspector: WorkbenchInspector | null = shown && thread ? {
    id: shown,
    title: shown === 'git' ? 'Git' : et('title'),
    subtitle: thread.title,
    actions: gitAvailable && isolationAvailable ? <InspectorSwitch value={shown} onChange={setView} isolationLabel={isolationLabel} attention={attention ? requestMark(attention.state) : undefined} /> : undefined,
    // Keyed by device and thread: switching threads never shows the previous
    // thread's diff, request or decision target.
    content: shown === 'git'
      ? <ThreadGitPanel key={`git:${scope}`} threadId={thread.id} deviceId={deviceId} refreshKey={thread.updatedAt} />
      : <ThreadIsolationPanel key={`isolation:${scope}`} thread={thread} deviceId={deviceId} canManage={canManage} />,
    onClose: () => setView(null),
  } : null;
  return { toolPanels, inspector };
}

/** Switch the dock between Git and isolation without leaving it. */
function InspectorSwitch({ value, onChange, isolationLabel, attention }: { value: InspectorView; onChange(view: InspectorView): void; isolationLabel: string; attention: string | undefined }) {
  return <div className="inspector-switch" role="group" aria-label={et('inspectorViews')}>
    <button type="button" aria-pressed={value === 'git'} aria-label="Git" title="Git" onClick={() => onChange('git')}><GitBranch aria-hidden="true" /></button>
    <button type="button" aria-pressed={value === 'isolation'} aria-label={isolationLabel} title={isolationLabel} onClick={() => onChange('isolation')}>
      <span className="inspector-rail-icon" data-attention={attention}><Box aria-hidden="true" /></span>
    </button>
  </div>;
}
