import type { ThreadDetailDto } from '@pockymoe/shared';
import { CopyButton, PathChip, formatLongTimestamp, threadStatusLabel } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';

export function ThreadSessionDetails({ detail }: { detail: ThreadDetailDto }) {
  useI18n();
  return (
    <dl className="space-y-4 text-sm">
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.pockymoeSessionID")}</dt>
        <dd className="mt-1 break-all text-[var(--theme-fg)]">{detail.thread.id}<CopyButton value={detail.thread.id} label={translate("workbench.copyPockymoeSessionID")} /></dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.harnessSessionID")}</dt>
        <dd className="mt-1 flex items-center gap-2 break-all">{detail.thread.providerSessionId ?? translate("workbench.unavailable")}{detail.thread.providerSessionId && <CopyButton value={detail.thread.providerSessionId} label={translate("workbench.copyHarnessSessionID")} />}</dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.source")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.thread.source === 'supervisor'
            ? translate("workbench.supervisorThread", { value1: detail.thread.provider })
            : translate("workbench.importedLocalSession", { value1: detail.thread.provider })}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.status")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {threadStatusLabel(detail.thread.status)}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.created")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {formatLongTimestamp(detail.thread.createdAt)}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.workspace")}</dt>
        <dd className="mt-1 break-words text-[var(--theme-fg)]">
          <PathChip path={detail.workspace.absPath} />
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.workspacePath")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.workspacePathStatus === 'present' ? translate("workbench.present") : translate("workbench.missingOnThisMachine")}
        </dd>
      </div>
      <div>
        <dt className="text-[var(--theme-fg-muted)]">{translate("workbench.activeTurn")}</dt>
        <dd className="mt-1 text-[var(--theme-fg)]">
          {detail.thread.activeTurnId ?? translate("workbench.none")}
        </dd>
      </div>
    </dl>
  );
}
