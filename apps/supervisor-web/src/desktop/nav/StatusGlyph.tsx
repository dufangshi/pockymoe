import { failed, running, unread, type DesktopThread } from '../data/types';
export function StatusGlyph({ thread }: { thread?: DesktopThread | undefined }) {
  const kind = !thread ? 'idle' : thread.requests.length ? 'attention' : failed(thread) ? 'failed' : running(thread) ? 'running' : unread(thread) ? 'unread' : thread.closedAt ? 'closed' : 'idle';
  return <svg className={`rc-status rc-status-${kind}`} width="14" height="14" viewBox="0 0 14 14" aria-label={kind === 'attention' ? '需要你' : kind === 'running' ? '运行中' : kind === 'failed' ? '失败' : kind === 'unread' ? '未读' : kind === 'closed' ? '已关闭' : undefined} aria-hidden={kind === 'idle'}>
    {kind === 'attention' ? <><circle className="rc-attention-halo" cx="7" cy="7" r="6" fill="currentColor" /><circle cx="7" cy="7" r="3" fill="currentColor" /></> : kind === 'running' ? <path d="M7 1.7a5.3 5.3 0 1 1-5.3 5.3" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" /> : kind === 'failed' ? <path d="m7 1 6 6-6 6-6-6z" fill="currentColor" /> : kind === 'unread' ? <circle cx="7" cy="7" r="3" fill="currentColor" /> : kind === 'closed' ? <circle cx="7" cy="7" r="3.5" stroke="currentColor" fill="none" /> : null}
  </svg>;
}
