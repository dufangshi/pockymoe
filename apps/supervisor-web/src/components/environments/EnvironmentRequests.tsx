import { useCallback, useEffect, useRef, useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { ErrorMark, StatusMark, formatShortTimestamp } from '@pockymoe/thread-ui';
import { useI18n } from '@pockymoe/thread-ui/i18n';
import { fetchEnvironmentRequests, notifyEnvironmentChanged, type EnvironmentRequestDto } from '../../lib/environmentApi';
import { environmentRequestText as t } from '../../lib/environmentRequestI18n';
import { RequestReview } from './RequestReview';
import { SectionHeading, Tag, requestMark } from './ui';
import './environments.css';

/** Pending first, then newest. */
const order = (a: EnvironmentRequestDto, b: EnvironmentRequestDto) =>
  Number(b.state === 'pending') - Number(a.state === 'pending') || b.createdAt.localeCompare(a.createdAt);

/** Requests for a device or one thread. The list polls while it is shown, so an
    approval's progress (waiting → migrating → isolated) appears without a reload. */
export function useEnvironmentRequests(deviceId: string | null | undefined, threadId: string | undefined, onChanged?: () => void) {
  const [requests, setRequests] = useState<EnvironmentRequestDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const previous = useRef(new Map<string, string>());
  const changed = useRef(onChanged); changed.current = onChanged;
  useEffect(() => {
    const controller = new AbortController(); let polling = false;
    const load = async () => {
      if (polling) return; polling = true;
      try {
        const next = await fetchEnvironmentRequests(deviceId, threadId, controller.signal);
        if (controller.signal.aborted) return;
        for (const request of next) {
          const before = previous.current.get(request.id);
          if (before && before !== request.state) { notifyEnvironmentChanged(request.threadId, deviceId); changed.current?.(); }
        }
        previous.current = new Map(next.map(request => [request.id, request.state]));
        setRequests(next); setError(null);
      } catch { if (!controller.signal.aborted) setError(t('loadFailed')); }
      finally { polling = false; if (!controller.signal.aborted) setLoading(false); }
    };
    void load(); const timer = window.setInterval(() => void load(), 3000);
    return () => { controller.abort(); window.clearInterval(timer); };
  }, [deviceId, threadId, revision]);
  const update = useCallback((next: EnvironmentRequestDto) => {
    setRequests(current => current.map(request => request.id === next.id ? next : request));
    previous.current.set(next.id, next.state); notifyEnvironmentChanged(next.threadId, deviceId); changed.current?.();
  }, [deviceId]);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  return { requests: requests.slice().sort(order), loading, error, refresh, update };
}

export function RequestList({ requests, showThread, onOpen }: { requests: EnvironmentRequestDto[]; showThread: boolean; onOpen(id: string): void }) {
  return <ul className="env-list" data-testid="environment-request-list">
    {requests.map(request => <li key={request.id} data-request-id={request.id}>
      <button type="button" className="env-row env-request-row" data-state={request.state} onClick={() => onOpen(request.id)}>
        <StatusMark status={requestMark(request.state)} label={t(request.state)} />
        <span className="env-row-main">
          <span className="env-row-title">{request.spec.name}</span>
          <span className="env-row-meta">
            <span>{t(request.state)}</span>
            <span>{request.spec.connection.name}</span>
            {showThread && <code title={request.threadId}>{request.threadId.slice(0, 8)}</code>}
            <time dateTime={request.createdAt}>{formatShortTimestamp(request.createdAt)}</time>
          </span>
        </span>
        {request.state === 'pending' ? <Tag tone="warning">{t('review')}</Tag> : null}
        <ChevronRight aria-hidden="true" className="env-row-chevron" />
      </button>
    </li>)}
  </ul>;
}

/** Settings and tests: the request list with its own review window. */
export function EnvironmentRequests({ deviceId, threadId, canManage = true, onChanged, hideWhenEmpty = false }: {
  deviceId?: string | null | undefined; threadId?: string; canManage?: boolean; onChanged?(): void; hideWhenEmpty?: boolean;
}) {
  useI18n();
  const { requests, loading, error, refresh, update } = useEnvironmentRequests(deviceId, threadId, onChanged);
  const [openId, setOpenId] = useState<string | null>(null);
  const open = requests.find(request => request.id === openId);
  const pending = requests.filter(request => request.state === 'pending').length;
  if (hideWhenEmpty && !loading && !error && !requests.length) return null;
  return <section className="env-section" aria-label={t('requests')} data-testid="environment-requests">
    <SectionHeading title={t('requests')} count={pending || undefined}>
      {error && <ErrorMark message={error} onRetry={refresh} />}
    </SectionHeading>
    {loading && !requests.length && <p role="status" className="env-empty">{t('loading')}</p>}
    {!loading && !error && !requests.length && <p className="env-empty">{t('empty')}</p>}
    {!!requests.length && <RequestList requests={requests} showThread={!threadId} onOpen={setOpenId} />}
    {open && <RequestReview request={open} deviceId={deviceId} canManage={canManage} onUpdate={update} onClose={() => setOpenId(null)} />}
  </section>;
}
