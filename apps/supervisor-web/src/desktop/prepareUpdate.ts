import { confirmAsync, getDirtyWorkspaceDocumentCount, prepareWorkspaceDocumentUnloadApproval, revokeWorkspaceDocumentUnloadApproval } from '@pockymoe/thread-ui';
import { getLocale, translate } from '@pockymoe/thread-ui/i18n';
import { getThreadDraftRevision, getUnsentThreadDraftCounts, getUnsentThreadDraftTitles } from '../pages/useThreadDrafts';
import type { HostCommandResult } from './host';

export interface UpdatePreparationDependencies {
  draftRevision(): number;
  draftCounts(): { drafts: number; attachments: number };
  draftTitles?(): (string | undefined)[];
  documentCount(): number;
  prepareDocuments(): () => boolean;
}
const defaults: UpdatePreparationDependencies = { draftRevision: getThreadDraftRevision, draftCounts: getUnsentThreadDraftCounts, draftTitles: getUnsentThreadDraftTitles, documentCount: getDirtyWorkspaceDocumentCount, prepareDocuments: prepareWorkspaceDocumentUnloadApproval };
let approvedDraftRevision: { revision: number; read(): number } | null = null;
let generation = 0;
let activeConfirmation: AbortController | null = null;
let pending: Promise<HostCommandResult> | null = null;
const cancelled = (): HostCommandResult => ({ ok: false, error: translate('desktopShell.updateCancelled') });

window.addEventListener('beforeunload', event => {
  if (approvedDraftRevision === null) return;
  if (approvedDraftRevision.revision !== approvedDraftRevision.read()) {
    event.preventDefault(); event.returnValue = '';
  } else if (event.isTrusted) approvedDraftRevision = null;
});

/** Native asks every trusted window, including requests from the tray. The rail
 * only calls install(); it must not open an additional confirmation itself. */
export function prepareClientUpdate(cancel = false, dependencies: UpdatePreparationDependencies = defaults): Promise<HostCommandResult> {
  if (cancel) {
    generation++;
    activeConfirmation?.abort(); activeConfirmation = null;
    approvedDraftRevision = null;
    revokeWorkspaceDocumentUnloadApproval();
    pending = null;
    return Promise.resolve({ ok: true });
  }
  if (pending) return pending;
  const current = ++generation;
  revokeWorkspaceDocumentUnloadApproval(); approvedDraftRevision = null;
  const draftRevision = dependencies.draftRevision();
  const counts = dependencies.draftCounts();
  const documents = dependencies.documentCount();
  const approveDocuments = dependencies.prepareDocuments();
  const controller = new AbortController(); activeConfirmation = controller;
  const work = async (): Promise<HostCommandResult> => {
    if (counts.drafts || documents) {
      const titles = (dependencies.draftTitles?.() ?? []).map(title => title || translate('workbench.splitUntitled'));
      const description = counts.drafts
        ? `${translate('desktopShell.updatePendingDescription', { titles: new Intl.ListFormat(getLocale(), { style: 'long', type: 'conjunction' }).format(titles.length ? titles : [translate('workbench.splitUntitled')]) })}
${translate('desktopShell.updatePendingCounts', { ...counts, documents })}`
        : translate('desktopShell.updatePendingDocuments', { documents });
      // The update action uses primary tone; the text explicitly names the work
      // that will be lost, while Cancel remains the initial focused control.
      const accepted = await confirmAsync({ title: translate('desktopShell.updatePendingTitle'), description, confirmLabel: translate('desktopShell.updateDiscardConfirm'), tone: 'primary', signal: controller.signal });
      if (!accepted) return cancelled();
    }
    if (current !== generation || draftRevision !== dependencies.draftRevision() || !approveDocuments()) return cancelled();
    approvedDraftRevision = { revision: draftRevision, read: dependencies.draftRevision };
    // Older pages acknowledge unknown host commands with plain ok:true. Native
    // requires this marker so only a page with the guard can approve closing.
    return { ok: true, data: { prepared: true } };
  };
  pending = work().finally(() => { if (current === generation) { activeConfirmation = null; pending = null; } });
  return pending;
}
