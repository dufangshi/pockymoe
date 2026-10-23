import { CopyButton, ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { RelayAccessGrantDto, RelayPortalSummaryDto, RelaySessionShareDto } from '@pockymoe/shared';
import { Folder, Hourglass, MessageSquare, Monitor, Send, ShieldCheck } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { ProductHeader } from '../components/ProductHeader';
import { currentAccount, safetyCode } from '../lib/accountVault';
import { enableRelayMode, fetchRelayPortal, setSelectedRelayDeviceId, setSelectedRelayThreadId } from '../lib/api';
import { threadHref, threadsHref, workspacesHref } from '../lib/relayRoutes';
import './share-pending.css';

const POLL_MS = 4000;
const SHARING_HREF = '/relay-devices?tab=shared';

type Received = { kind: 'share'; row: RelaySessionShareDto } | { kind: 'grant'; row: RelayAccessGrantDto };

/** `sharedThreadsWithMe` repeats thread shares without a grant `scope`; only
    rows with one are grants. */
function findReceived(portal: RelayPortalSummaryDto, kind: string, id: string): Received | null {
  if (kind === 'share') {
    const row = portal.sharedWithMe.find(entry => entry.id === id);
    return row ? { kind, row } : null;
  }
  const row = [...(portal.sharedDevicesWithMe ?? []), ...(portal.sharedThreadsWithMe ?? [])]
    .find(entry => entry.id === id && typeof (entry as Partial<RelayAccessGrantDto>).scope === 'string') as RelayAccessGrantDto | undefined;
  return row ? { kind: 'grant', row } : null;
}

function targetHref({ kind, row }: Received) {
  if (row.threadId) return threadHref(row.threadId, row.deviceId);
  if (kind === 'grant' && row.scope === 'workspace' && row.workspaceId) return threadsHref(row.workspaceId, row.deviceId);
  return workspacesHref(row.deviceId);
}

/** The recipient's side of the safety-code check
    (docs/account-key-trust.md#safety-code-check): who shared what, their own
    safety code to send, and the share opens by itself once the owner confirms. */
export function SharePendingPage() {
  useI18n();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const kind = params.get('kind') ?? '';
  const id = params.get('id') ?? '';
  const [received, setReceived] = useState<Received | null>();
  const [gone, setGone] = useState(false);
  const [loadError, setLoadError] = useState<string>();
  const [attempt, setAttempt] = useState(0);
  const [code, setCode] = useState<string | null>();
  const confirmed = Boolean(received) && received?.row.pending !== true;

  useEffect(() => {
    let active = true;
    void currentAccount()
      .then(async account => { const next = account ? await safetyCode(account.accountKeyId) : null; if (active) setCode(next); })
      .catch(() => { if (active) setCode(null); });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    enableRelayMode();
    const poll = async () => {
      timer = undefined;
      try {
        const portal = await fetchRelayPortal();
        if (!active) return;
        const found = findReceived(portal, kind, id);
        setReceived(found);
        setGone(!found);
        setLoadError(undefined);
        if (found && found.row.pending !== true) return;
      } catch (error) {
        // A failed refresh keeps what is shown; only a first load says why.
        if (active) setLoadError(error instanceof Error ? error.message : translate('devices.unableToLoadDevices'));
      }
      if (active && document.visibilityState !== 'hidden') timer = window.setTimeout(() => void poll(), POLL_MS);
    };
    const onVisible = () => {
      if (document.visibilityState === 'visible' && timer === undefined) void poll();
    };
    void poll();
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      active = false;
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [kind, id, attempt]);

  useEffect(() => {
    if (!received || !confirmed) return;
    // A moment to read "Confirmed" before the share opens.
    const timer = window.setTimeout(() => {
      setSelectedRelayDeviceId(received.row.deviceId);
      setSelectedRelayThreadId(received.row.threadId ?? null);
      navigate(targetHref(received), { replace: true });
    }, 700);
    return () => window.clearTimeout(timer);
  }, [received, confirmed, navigate]);

  const row = received?.row;
  const owner = row?.ownerUsername || translate('sharing.pendingSomeone');
  const device = row?.deviceName || row?.deviceId.slice(0, 8) || '';
  const scope = received?.kind === 'grant' ? received.row.scope : 'thread';
  const title = scope === 'workspace'
    ? row?.workspaceLabel || row?.workspaceId?.slice(0, 8) || ''
    : row?.threadTitle || row?.label || row?.threadId?.slice(0, 8) || '';
  const Icon = scope === 'device' ? Monitor : scope === 'workspace' ? Folder : MessageSquare;
  const shareText = code ? translate('sharing.pendingShareText', { code }) : '';
  const canShare = typeof navigator !== 'undefined' && typeof navigator.share === 'function';

  return (
    <div className="product-page !max-w-2xl" data-page="share-pending">
      <ProductHeader title={translate('sharing.tabSharing')} backHref={SHARING_HREF} backLabel={translate('sharing.pendingBack')} />
      <main className="share-pending">
        {received === undefined && !loadError ? (
          <p className="share-pending-status" role="status">{translate('sharing.loadingShares')}</p>
        ) : received === undefined && loadError ? (
          <p className="share-pending-status">
            {translate('devices.unavailable')}
            <ErrorMark message={loadError} onRetry={() => { setLoadError(undefined); setAttempt(value => value + 1); }} />
          </p>
        ) : gone || !row ? (
          <section className="share-pending-card">
            <p>{translate('sharing.pendingGone', { owner: translate('sharing.pendingSomeone') })}</p>
            <Link className="share-pending-link" to={SHARING_HREF}>{translate('sharing.pendingBack')}</Link>
          </section>
        ) : (
          <section className="share-pending-card" aria-labelledby="share-pending-title">
            <div className="share-pending-head">
              <span className="share-pending-mark" data-confirmed={confirmed || undefined}>
                {confirmed ? <ShieldCheck size={20} aria-hidden="true" /> : <Hourglass size={20} aria-hidden="true" />}
              </span>
              <h1 id="share-pending-title">{confirmed ? translate('sharing.pendingConfirmed') : translate('sharing.pendingPageTitle', { name: owner })}</h1>
            </div>
            <p className="share-pending-what">
              <Icon size={15} aria-hidden="true" />
              <span>{scope === 'device'
                ? translate('sharing.pendingWhatDevice', { owner, device })
                : translate(scope === 'workspace' ? 'sharing.pendingWhatWorkspace' : 'sharing.pendingWhatThread', { owner, title, device })}</span>
            </p>
            {code ? (
              <div className="share-pending-code-box">
                <p className="share-pending-code-label">{translate('sharing.pendingYourCode')}</p>
                <div className="share-pending-code-line">
                  <code className="share-pending-code">{code}</code>
                  <CopyButton value={code} label={translate('sharing.copyCode')} className="share-pending-copy" />
                </div>
                {canShare ? (
                  <button type="button" className="relay-button-secondary share-pending-send" onClick={() => void navigator.share({ text: shareText }).catch(() => {})}>
                    <Send size={15} aria-hidden="true" />{translate('sharing.pendingShareCode')}
                  </button>
                ) : null}
              </div>
            ) : code === null ? (
              <p className="share-pending-note">
                {translate('sharing.pendingNoAccount')}{' '}
                <Link className="share-pending-link" to="/relay-account">{translate('auth.accountSettings')}</Link>
              </p>
            ) : null}
            {!confirmed ? (
              <>
                <p className="share-pending-explain">{translate('sharing.pendingExplain', { owner })}</p>
                <p className="share-pending-status" role="status">
                  <span className="share-pending-pulse" aria-hidden="true" />
                  {translate('sharing.pendingWaiting', { owner })}
                </p>
              </>
            ) : null}
          </section>
        )}
      </main>
    </div>
  );
}
