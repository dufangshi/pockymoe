import { useEffect, useRef, type RefObject } from 'react';
import type { ThreadViewMessage } from '@pockymoe/shared';

/** A hidden tab stops counting as a viewer after this long. */
export const THREAD_VIEW_HIDDEN_MS = 60_000;
const SOCKET_OPEN = 1;

/**
 * Tells one Supervisor socket which threads this page displays, so the
 * Supervisor keeps their harnesses loaded. Sends the full set whenever it
 * changes, an empty set once the tab has been hidden for a minute, and the set
 * again when the tab returns or a new socket opens.
 */
export class ThreadViewReporter {
  private threadIds: string[] = [];
  private hiddenLongEnough = false;
  private hiddenTimer: ReturnType<typeof setTimeout> | null = null;
  private sentOn: WebSocket | null = null;
  private sent = '';

  constructor(
    private readonly socket: () => WebSocket | null,
    private readonly hiddenMs = THREAD_VIEW_HIDDEN_MS,
  ) {}

  setThreads(threadIds: Iterable<string>) {
    this.threadIds = [...new Set(threadIds)].filter(Boolean).sort();
    this.flush();
  }

  /** Call when a socket opens: a new connection starts with no viewers. */
  socketOpened() {
    this.sentOn = null;
    this.flush();
  }

  setHidden(hidden: boolean) {
    if (hidden) {
      if (this.hiddenTimer === null && !this.hiddenLongEnough)
        this.hiddenTimer = setTimeout(() => {
          this.hiddenTimer = null;
          this.hiddenLongEnough = true;
          this.flush();
        }, this.hiddenMs);
      return;
    }
    this.clearTimer();
    this.hiddenLongEnough = false;
    this.flush();
  }

  dispose() {
    this.clearTimer();
  }

  private clearTimer() {
    if (this.hiddenTimer !== null) clearTimeout(this.hiddenTimer);
    this.hiddenTimer = null;
  }

  private flush() {
    const socket = this.socket();
    if (!socket || socket.readyState !== SOCKET_OPEN) return;
    const message: ThreadViewMessage = {
      type: 'thread.view',
      threadIds: this.hiddenLongEnough ? [] : this.threadIds,
    };
    const body = JSON.stringify(message);
    if (socket === this.sentOn && body === this.sent) return;
    // A new connection already starts with nothing displayed.
    const nothingNew = socket !== this.sentOn && !message.threadIds.length;
    if (nothingNew) {
      this.sentOn = socket;
      this.sent = body;
      return;
    }
    try {
      socket.send(body);
      this.sentOn = socket;
      this.sent = body;
    } catch {
      // The socket's own close handling reconnects; the next open resends.
    }
  }
}

/**
 * Reports `threadIds` on the socket held in `socketRef`. Call the returned
 * reporter's `socketOpened()` from the socket's open handler.
 */
export function useThreadView(socketRef: RefObject<WebSocket | null>, threadIds: string[]) {
  const reporterRef = useRef<ThreadViewReporter | null>(null);
  reporterRef.current ??= new ThreadViewReporter(() => socketRef.current);
  const reporter = reporterRef.current;
  const key = [...threadIds].sort().join('\n');
  useEffect(() => {
    reporter.setThreads(key ? key.split('\n') : []);
  }, [reporter, key]);
  useEffect(() => {
    const sync = () => reporter.setHidden(document.visibilityState === 'hidden');
    sync();
    document.addEventListener('visibilitychange', sync);
    return () => {
      document.removeEventListener('visibilitychange', sync);
      reporter.dispose();
    };
  }, [reporter]);
  return reporter;
}
