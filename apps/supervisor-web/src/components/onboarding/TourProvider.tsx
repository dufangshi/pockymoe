import { devicesHref } from '../../lib/devicesRoute';
import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { Check, CircleHelp, Compass, X } from 'lucide-react';
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { useLocation, useNavigate } from 'react-router-dom';

import { fetchAuthSession, fetchRelaySession, relayModeActive } from '../../lib/api';
import { onTourRoute, resetTourSandbox, TOUR_DEVICE_ID, TOUR_HELPER_THREAD, TOUR_MAIN_THREAD } from '../../lib/tourSandbox/tourDevice';
import {
  TOUR_CHAPTER_IDS,
  resolveSelector,
  tourChapter,
  type TourChapterId,
  type TourContext,
  type TourLayout,
  type TourPage,
  type TourStep,
  type TourTargetVariant,
  type TourThread,
} from './tourChapters';
import { placeCard, sameRect, unionRect, type TourRect } from './tourPlacement';
import './tour.css';
import { EMPTY_PROGRESS, progressStorageKey, readProgress, writeProgress, type TourProgress } from './tourStorage';

type TourView = { kind: 'closed' } | { kind: 'hub' } | { kind: 'step'; chapter: TourChapterId; index: number };

interface TourApi {
  openHub: () => void;
  startChapter: (chapter: TourChapterId) => void;
}

const TourApiContext = createContext<TourApi | null>(null);

export function useTour() {
  return useContext(TourApiContext);
}

/** Grace period before a missing control is reported, so panels can mount after a click. */
const TARGET_GRACE_MS = 700;
const MOBILE_QUERY = '(max-width: 639px)';
const ROOT_ID = 'pockymoe-tour-root';

export function pageFromPath(pathname: string): TourPage | null {
  const path = pathname.replace(/^\/devices\/[^/]+(?=\/)/, '');
  if (path === '/workspaces') return 'workspaces';
  if (path === '/workspaces/new') return 'workspaceNew';
  if (path === '/threads/import') return 'threadImport';
  if (path === '/relay-devices' || path === '/desktop/devices' || path.startsWith('/desktop/devices/')) return 'relayDevices';
  if (/^\/threads\/[^/]+$/.test(path) && path !== '/threads/new') return 'thread';
  return null;
}

const TOUR_THREADS: Record<TourThread, string> = { main: TOUR_MAIN_THREAD, helper: TOUR_HELPER_THREAD };
const TOUR_BASE = `/devices/${TOUR_DEVICE_ID}`;

/** Where a step is: the sample device's pages, or the account's device list. */
export function tourPageFromPath(pathname: string): { page: TourPage; threadId: string | null } | null {
  if (pageFromPath(pathname) === 'relayDevices') return { page: 'relayDevices', threadId: null };
  if (!pathname.startsWith(`${TOUR_BASE}/`)) return null;
  const page = pageFromPath(pathname);
  return page ? { page, threadId: page === 'thread' ? decodeURIComponent(pathname.split('/').pop() ?? '') : null } : null;
}

function tourHref(page: TourPage, thread: TourThread = 'main') {
  switch (page) {
    case 'relayDevices': return devicesHref();
    case 'workspaces': return `${TOUR_BASE}/workspaces`;
    case 'workspaceNew': return `${TOUR_BASE}/workspaces/new`;
    case 'threadImport': return `${TOUR_BASE}/threads/import`;
    case 'thread': return `${TOUR_BASE}/threads/${TOUR_THREADS[thread]}`;
  }
}

const VIEW_KEY = 'pockymoe.onboarding.view';

function readView(): TourView {
  try {
    const value = JSON.parse(window.sessionStorage.getItem(VIEW_KEY) ?? 'null') as TourView | null;
    if (value?.kind === 'hub') return value;
    if (value?.kind === 'step' && TOUR_CHAPTER_IDS.includes(value.chapter) && Number.isInteger(value.index)) return value;
  } catch { /* A broken value starts closed. */ }
  return { kind: 'closed' };
}

function writeView(view: TourView) {
  try {
    if (view.kind === 'closed') window.sessionStorage.removeItem(VIEW_KEY);
    else window.sessionStorage.setItem(VIEW_KEY, JSON.stringify(view));
  } catch { /* The tour still works for this visit. */ }
}

// The page the reader opened the tour from, to return to when it closes.
const RETURN_KEY = 'pockymoe.onboarding.return';

function rememberReturn(href: string) {
  try {
    if (!window.sessionStorage.getItem(RETURN_KEY)) window.sessionStorage.setItem(RETURN_KEY, href);
  } catch { /* Without storage, closing the tour stays on the sample page. */ }
}

function takeReturn() {
  try {
    const href = window.sessionStorage.getItem(RETURN_KEY);
    window.sessionStorage.removeItem(RETURN_KEY);
    return href;
  } catch {
    return null;
  }
}

function useLayout(): TourLayout {
  const [mobile, setMobile] = useState(() => typeof window !== 'undefined' && window.matchMedia(MOBILE_QUERY).matches);
  useEffect(() => {
    const query = window.matchMedia(MOBILE_QUERY);
    const change = () => setMobile(query.matches);
    query.addEventListener('change', change);
    return () => query.removeEventListener('change', change);
  }, []);
  return mobile ? 'mobile' : 'desktop';
}

function isVisible(element: Element) {
  if (element.closest(`#${ROOT_ID}, [hidden], [inert]`)) return false;
  const rect = element.getBoundingClientRect();
  if (rect.width <= 0 || rect.height <= 0) return false;
  return getComputedStyle(element).visibility !== 'hidden';
}

function firstVisible(selector: string) {
  try {
    return Array.from(document.querySelectorAll(selector)).find(isVisible) ?? null;
  } catch {
    return null;
  }
}

/** Radix modals disable pointer events on <body>; the tour steps aside until they close. */
function foreignModalOpen() {
  return document.body.style.pointerEvents === 'none';
}

function toRect(element: Element): TourRect {
  const rect = element.getBoundingClientRect();
  return { top: rect.top, left: rect.left, width: rect.width, height: rect.height };
}

function isTypingTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  return Boolean(target.closest('input, textarea, select, [contenteditable="true"], [role="textbox"], .xterm'));
}

interface TrackedTarget {
  element: Element;
  rect: TourRect;
  variant: TourTargetVariant | null;
}

function findTarget(step: TourStep): TrackedTarget | null {
  for (const variant of step.targets ?? []) {
    const element = firstVisible(resolveSelector(variant.selector));
    if (element) return { element, rect: toRect(element), variant };
  }
  if (step.prerequisiteTarget) {
    const element = firstVisible(resolveSelector(step.prerequisiteTarget));
    if (element) return { element, rect: toRect(element), variant: null };
  }
  return null;
}

function useTourRoot() {
  const [root, setRoot] = useState<HTMLElement | null>(null);
  useEffect(() => {
    const element = document.createElement('div');
    element.id = ROOT_ID;
    // Menus such as the split picker close on outside pointerdown; tour clicks must not close them.
    const isolate = (event: Event) => event.stopPropagation();
    for (const type of ['pointerdown', 'mousedown', 'touchstart']) element.addEventListener(type, isolate);
    document.body.appendChild(element);
    setRoot(element);
    return () => {
      element.remove();
    };
  }, []);
  return root;
}

function useAccount(mode: TourContext['mode']) {
  const location = useLocation();
  const [account, setAccount] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    const load =
      mode === 'relay'
        ? fetchRelaySession().then((session) => (session.user?.id ? `relay:${session.user.id}` : null))
        : fetchAuthSession().then((session) => session.authenticated ? `local:${session.username ?? 'owner'}` : null);
    load
      .then((value) => {
        if (!cancelled) setAccount(value);
      })
      .catch(() => {
        if (!cancelled) setAccount(null);
      });
    return () => {
      cancelled = true;
    };
  }, [mode, location.pathname]);
  return account;
}

export function TourProvider({ children }: { children: ReactNode }) {
  useI18n();
  const location = useLocation();
  const navigate = useNavigate();
  const layout = useLayout();
  const mode = relayModeActive() ? 'relay' : 'local';
  const context = useMemo<TourContext>(() => ({ layout, mode }), [layout, mode]);
  const account = useAccount(mode);
  const storageKey = account ? progressStorageKey(account) : null;
  const [progress, setProgress] = useState<TourProgress>(EMPTY_PROGRESS);
  const [progressLoaded, setProgressLoaded] = useState(false);
  // The open chapter and step survive a reload or remount, like the page they show.
  const [view, setViewState] = useState<TourView>(readView);
  const setView = useCallback((next: TourView) => {
    setViewState(next);
    writeView(next);
  }, []);
  const root = useTourRoot();
  const pathname = location.pathname;
  const page = pageFromPath(pathname);
  const here = tourPageFromPath(pathname);

  useEffect(() => {
    setProgress(storageKey ? readProgress(storageKey) : EMPTY_PROGRESS);
    setProgressLoaded(Boolean(storageKey));
  }, [storageKey]);

  const updateProgress = useCallback(
    (change: (current: TourProgress) => TourProgress) => {
      setProgress((current) => {
        const next = change(current);
        if (storageKey) writeProgress(storageKey, next);
        return next;
      });
    },
    [storageKey],
  );

  // A tour starts on the sample device from where the reader is, and returns there.
  const begin = useCallback(() => {
    if (onTourRoute()) return;
    rememberReturn(`${location.pathname}${location.search}${location.hash}`);
    void resetTourSandbox();
  }, [location.hash, location.pathname, location.search]);

  const api = useMemo<TourApi>(
    () => ({
      openHub: () => {
        updateProgress((current) => ({ ...current, welcomeDismissed: true }));
        begin();
        setView({ kind: 'hub' });
      },
      startChapter: (chapter) => {
        updateProgress((current) => ({ ...current, welcomeDismissed: true }));
        begin();
        setView({ kind: 'step', chapter, index: 0 });
      },
    }),
    [begin, updateProgress],
  );

  const chapterId = view.kind === 'step' ? view.chapter : null;
  const chapter = useMemo(() => (chapterId ? tourChapter(chapterId, context) : null), [chapterId, context]);
  const step = chapter && view.kind === 'step' ? chapter.steps[Math.min(view.index, chapter.steps.length - 1)] ?? null : null;
  const stepKey = chapter && step ? `${chapter.id}:${step.id}:${layout}` : null;

  // Remember where an unfinished chapter stopped.
  useEffect(() => {
    if (!chapter || !step) return;
    updateProgress((current) =>
      current.resume[chapter.id] === step.id ? current : { ...current, resume: { ...current.resume, [chapter.id]: step.id } },
    );
  }, [chapter, step, updateProgress]);

  const onStepPage = (target: TourStep) => !target.page || (here?.page === target.page
    && (target.page !== 'thread' || here.threadId === TOUR_THREADS[target.thread ?? 'main']));
  const stepHref = step?.page ? tourHref(step.page, step.thread) : null;
  const stepMismatch = Boolean(step && !onStepPage(step));

  // Each step opens its page once; the reader can still navigate away.
  const navigatedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!stepKey || !stepMismatch || !stepHref || navigatedFor.current === stepKey) return;
    navigatedFor.current = stepKey;
    navigate(stepHref);
  }, [navigate, stepHref, stepKey, stepMismatch]);

  // Closing the tour leaves the sample device for the page it started from.
  const close = useCallback(() => {
    setView({ kind: 'closed' });
    const back = takeReturn();
    if (onTourRoute()) navigate(back && !back.startsWith(TOUR_BASE) ? back : '/');
  }, [navigate]);
  const goTo = useCallback(
    (index: number) => {
      if (!chapter || view.kind !== 'step') return;
      if (index < 0) return;
      if (index >= chapter.steps.length) {
        updateProgress((current) => {
          const resume = { ...current.resume };
          delete resume[chapter.id];
          return {
            ...current,
            completed: current.completed.includes(chapter.id) ? current.completed : [...current.completed, chapter.id],
            resume,
          };
        });
        setView({ kind: 'hub' });
        return;
      }
      setView({ kind: 'step', chapter: chapter.id, index });
    },
    [chapter, updateProgress, view.kind],
  );

  const showWelcome =
    progressLoaded &&
    !progress.welcomeDismissed &&
    view.kind === 'closed' &&
    (page === 'workspaces' || page === 'thread' || page === 'relayDevices');

  let overlay: ReactNode = null;
  if (view.kind === 'hub') {
    overlay = (
      <TourHub
        context={context}
        progress={progress}
        onClose={close}
        onStart={(id) => {
          const steps = tourChapter(id, context).steps;
          const resumeAt = progress.completed.includes(id) ? -1 : steps.findIndex((item) => item.id === progress.resume[id]);
          setView({ kind: 'step', chapter: id, index: Math.max(0, resumeAt) });
        }}
      />
    );
  } else if (chapter && step && view.kind === 'step' && stepKey) {
    overlay = (
      <TourStepOverlay
        key={stepKey}
        chapterNumber={TOUR_CHAPTER_IDS.indexOf(chapter.id) + 1}
        chapterTitle={chapter.title}
        step={step}
        index={Math.min(view.index, chapter.steps.length - 1)}
        total={chapter.steps.length}
        pageMismatch={stepMismatch}
        pageAction={stepMismatch && stepHref ? { label: 'tour.goThere', run: () => navigate(stepHref) } : null}
        pagePrerequisite={stepMismatch ? 'tour.elsewhere' : null}
        onBack={() => goTo(view.index - 1)}
        onNext={() => goTo(view.index + 1)}
        onContents={() => setView({ kind: 'hub' })}
        onClose={close}
      />
    );
  } else if (showWelcome) {
    overlay = (
      <TourWelcome
        onStart={api.openHub}
        onLater={() => updateProgress((current) => ({ ...current, welcomeDismissed: true }))}
      />
    );
  }

  return (
    <TourApiContext.Provider value={api}>
      {children}
      {root && overlay ? createPortal(<ModalAware>{overlay}</ModalAware>, root) : null}
    </TourApiContext.Provider>
  );
}

/** Hides tour UI while another modal dialog owns focus and pointer input. */
function ModalAware({ children }: { children: ReactNode }) {
  const [blocked, setBlocked] = useState(() => foreignModalOpen());
  useEffect(() => {
    const observer = new MutationObserver(() => setBlocked(foreignModalOpen()));
    observer.observe(document.body, { attributes: true, attributeFilter: ['style'] });
    setBlocked(foreignModalOpen()); // the modal may have closed before the observer started
    return () => observer.disconnect();
  }, []);
  return blocked ? null : <>{children}</>;
}

function useEscape(onEscape: () => void, cardRef: React.RefObject<HTMLElement | null>) {
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      const insideCard = event.target instanceof Node && cardRef.current?.contains(event.target);
      if (!insideCard && isTypingTarget(event.target)) return;
      onEscape();
    };
    document.addEventListener('keydown', keydown);
    return () => document.removeEventListener('keydown', keydown);
  }, [cardRef, onEscape]);
}

function usePlacedCard(target: TourRect | null, dock: 'top' | 'bottom' = 'top') {
  const cardRef = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState<{ top: number; left: number; placement: string } | null>(null);
  useLayoutEffect(() => {
    const card = cardRef.current;
    if (!card) return;
    const next = placeCard(target, { width: card.offsetWidth, height: card.offsetHeight }, { width: window.innerWidth, height: window.innerHeight }, dock);
    setPosition((current) =>
      current && current.top === next.top && current.left === next.left && current.placement === next.placement ? current : next,
    );
  });
  return { cardRef, position };
}

function TourStepOverlay({
  chapterNumber,
  chapterTitle,
  step,
  index,
  total,
  pageMismatch,
  pageAction,
  pagePrerequisite,
  onBack,
  onNext,
  onContents,
  onClose,
}: {
  chapterNumber: number;
  chapterTitle: TranslationKey;
  step: TourStep;
  index: number;
  total: number;
  pageMismatch: boolean;
  pageAction: { label: TranslationKey; run: () => void } | null;
  pagePrerequisite: TranslationKey | null;
  onBack: () => void;
  onNext: () => void;
  onContents: () => void;
  onClose: () => void;
}) {
  const [tracked, setTracked] = useState<TrackedTarget | null>(null);
  const [graceOver, setGraceOver] = useState(false);
  const scrolled = useRef(false);
  const shown = useRef(false);

  useEffect(() => {
    let frame = 0;
    const tick = () => {
      const found = pageMismatch ? null : findTarget(step);
      if (found?.variant && !scrolled.current) {
        const { rect } = found;
        if (rect.top < 0 || rect.left < 0 || rect.top + rect.height > window.innerHeight || rect.left + rect.width > window.innerWidth) {
          found.element.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        }
        scrolled.current = true;
      }
      setTracked((current) =>
        current?.element === found?.element && current?.variant === found?.variant && sameRect(current?.rect ?? null, found?.rect ?? null)
          ? current
          : found,
      );
      frame = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(frame);
  }, [pageMismatch, step]);

  const ready = Boolean(tracked?.variant);
  useEffect(() => {
    if (ready) return;
    const timer = window.setTimeout(() => setGraceOver(true), TARGET_GRACE_MS);
    return () => window.clearTimeout(timer);
  }, [ready]);
  // Once shown, the card stays and explains what is missing instead of vanishing.
  const visible = ready || graceOver || shown.current;
  shown.current = visible;

  // Clicking the real control advances, so the user learns by doing.
  const nextRef = useRef(onNext);
  nextRef.current = onNext;
  useEffect(() => {
    if (!step.advanceOnClick || !tracked?.variant) return;
    const element = tracked.element;
    const click = (event: MouseEvent) => {
      if (event.target instanceof Node && element.contains(event.target)) window.setTimeout(() => nextRef.current(), 0);
    };
    document.addEventListener('click', click, true);
    return () => document.removeEventListener('click', click, true);
  }, [step.advanceOnClick, tracked?.element, tracked?.variant]);

  const highlight = visible && tracked ? tracked.rect : null;
  const clear = ready && step.keepClear ? firstVisible(resolveSelector(step.keepClear)) : null;
  const { cardRef, position } = usePlacedCard(highlight && unionRect(highlight, clear ? toRect(clear) : null), 'bottom');
  useEscape(onClose, cardRef);

  // Keep keyboard users in the tour without stealing focus from the page.
  const primaryRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (!visible) return;
    const active = document.activeElement;
    if (!active || active === document.body || document.getElementById(ROOT_ID)?.contains(active)) primaryRef.current?.focus({ preventScroll: true });
  }, [visible]);

  if (!visible) return null;
  const prerequisite = pageMismatch ? pagePrerequisite : ready || step.optional ? null : step.prerequisite ?? 'tour.waiting';
  const body = tracked?.variant?.body ?? step.body;
  const last = index === total - 1;
  const titleId = 'pockymoe-tour-title';
  return (
    <>
      {highlight && (
        <div
          aria-hidden="true"
          className={`pm-tour-spotlight ${ready ? '' : 'is-prerequisite'}`}
          style={{ top: highlight.top - 6, left: highlight.left - 6, width: highlight.width + 12, height: highlight.height + 12 }}
        />
      )}
      <div
        ref={cardRef}
        role="dialog"
        aria-modal="false"
        aria-labelledby={titleId}
        className="pm-tour-card"
        data-placement={position?.placement}
        data-step={step.id}
        style={position ? { top: position.top, left: position.left } : { visibility: 'hidden', top: 0, left: 0 }}
        onKeyDown={(event) => {
          if (event.key === 'ArrowRight') onNext();
          else if (event.key === 'ArrowLeft') onBack();
        }}
      >
        <div className="pm-tour-card-header">
          <span className="pm-tour-eyebrow">{translate('tour.chapterLabel', { value1: chapterNumber, value2: translate(chapterTitle) })}</span>
          <span className="pm-tour-count">{translate('tour.stepOf', { value1: index + 1, value2: total })}</span>
          <button type="button" className="pm-tour-icon" aria-label={translate('tour.close')} title={translate('tour.close')} onClick={onClose}>
            <X aria-hidden="true" />
          </button>
        </div>
        <h2 id={titleId} className="pm-tour-title">{translate(step.title)}</h2>
        <p className="pm-tour-body">{translate(body)}</p>
        {prerequisite && (
          <p className="pm-tour-prerequisite" role="status">
            <strong>{translate('tour.prerequisite')}</strong>
            {translate(prerequisite)}
          </p>
        )}
        {ready && step.advanceOnClick && <p className="pm-tour-hint">{translate('tour.tryIt')}</p>}
        <div className="pm-tour-progress" aria-hidden="true">
          {Array.from({ length: total }, (_, dot) => (
            <span key={dot} className={dot === index ? 'is-current' : dot < index ? 'is-done' : ''} />
          ))}
        </div>
        <div className="pm-tour-actions">
          <button type="button" className="pm-tour-link" onClick={onContents}>{translate('tour.contents')}</button>
          <span className="pm-tour-spacer" />
          {pageAction && (
            <button type="button" className="pm-tour-secondary" onClick={pageAction.run}>{translate(pageAction.label)}</button>
          )}
          <button type="button" className="pm-tour-secondary" disabled={index === 0} onClick={onBack}>{translate('tour.back')}</button>
          <button ref={primaryRef} type="button" className="pm-tour-primary" onClick={onNext}>
            {last ? translate('tour.done') : translate('tour.next')}
          </button>
        </div>
      </div>
    </>
  );
}

function TourHub({
  context,
  progress,
  onStart,
  onClose,
}: {
  context: TourContext;
  progress: TourProgress;
  onStart: (chapter: TourChapterId) => void;
  onClose: () => void;
}) {
  const { cardRef, position } = usePlacedCard(null);
  useEscape(onClose, cardRef);
  const firstRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => firstRef.current?.focus({ preventScroll: true }), []);
  const next = TOUR_CHAPTER_IDS.find((id) => !progress.completed.includes(id));
  return (
    <div
      ref={cardRef}
      role="dialog"
      aria-modal="false"
      aria-labelledby="pockymoe-tour-hub-title"
      className="pm-tour-card pm-tour-hub"
      style={position ? { top: position.top, left: position.left } : { visibility: 'hidden', top: 0, left: 0 }}
    >
      <div className="pm-tour-card-header">
        <h2 id="pockymoe-tour-hub-title" className="pm-tour-title">{translate('tour.hubTitle')}</h2>
        <span className="pm-tour-spacer" />
        <button type="button" className="pm-tour-icon" aria-label={translate('tour.close')} title={translate('tour.close')} onClick={onClose}>
          <X aria-hidden="true" />
        </button>
      </div>
      <p className="pm-tour-body">{translate('tour.hubDescription')}</p>
      <ol className="pm-tour-chapters">
        {TOUR_CHAPTER_IDS.map((id, index) => {
          const chapter = tourChapter(id, context);
          const done = progress.completed.includes(id);
          const resumable = !done && Boolean(progress.resume[id]);
          return (
            <li key={id}>
              <button
                ref={id === (next ?? 'devices') ? firstRef : undefined}
                type="button"
                className={`pm-tour-chapter ${id === next ? 'is-next' : ''}`}
                onClick={() => onStart(id)}
              >
                <span className={`pm-tour-chapter-index ${done ? 'is-done' : ''}`} aria-hidden="true">
                  {done ? <Check /> : index + 1}
                </span>
                <span className="pm-tour-chapter-copy">
                  <span>{translate(chapter.title)}</span>
                  <small>
                    {translate(chapter.hint)} · {translate('tour.stepsCount', { count: chapter.steps.length })}
                    {done ? ` · ${translate('tour.completed')}` : ''}
                  </small>
                </span>
                <span className="pm-tour-chapter-action">
                  {done ? translate('tour.replay') : resumable ? translate('tour.resume') : translate('tour.start')}
                </span>
              </button>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

function TourWelcome({ onStart, onLater }: { onStart: () => void; onLater: () => void }) {
  const { cardRef, position } = usePlacedCard(null);
  useEscape(onLater, cardRef);
  return (
    <div
      ref={cardRef}
      role="dialog"
      aria-modal="false"
      aria-labelledby="pockymoe-tour-welcome-title"
      className="pm-tour-card pm-tour-welcome"
      style={position ? { top: position.top, left: position.left } : { visibility: 'hidden', top: 0, left: 0 }}
    >
      <div className="pm-tour-card-header">
        <CircleHelp aria-hidden="true" className="pm-tour-welcome-icon" />
        <h2 id="pockymoe-tour-welcome-title" className="pm-tour-title">{translate('tour.welcomeTitle')}</h2>
      </div>
      <p className="pm-tour-body">{translate('tour.welcomeBody')}</p>
      <div className="pm-tour-actions">
        <span className="pm-tour-spacer" />
        <button type="button" className="pm-tour-secondary" onClick={onLater}>{translate('tour.welcomeLater')}</button>
        <button type="button" className="pm-tour-primary" onClick={onStart}>{translate('tour.welcomeStart')}</button>
      </div>
    </div>
  );
}

/** Icon entry for the workbench rail. */
export function TourLauncherButton({ showLabel = false }: { showLabel?: boolean }) {
  useI18n();
  const tour = useTour();
  if (!tour) return null;
  return (
    <button type="button" className={`pm-tour-launcher ${showLabel ? 'pm-tour-launcher-labeled' : ''}`} aria-label={translate('tour.entry')} title={translate('tour.entry')} onClick={tour.openHub}>
      {showLabel ? <Compass data-icon="help" aria-hidden="true"/> : <CircleHelp data-icon="help" aria-hidden="true" />}
      {showLabel && <span>{translate('tour.entry')}</span>}
    </button>
  );
}

/** Settings → Preferences entry. Closes the settings dialog first so the tour is usable. */
export function TourSettingsEntry() {
  useI18n();
  const tour = useTour();
  if (!tour) return null;
  return (
    <fieldset className="py-5">
      <legend className="text-sm font-semibold text-[var(--theme-fg)]">{translate('tour.entry')}</legend>
      <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">{translate('tour.settingsHint')}</p>
      <button
        type="button"
        className="relay-button-secondary mt-3 inline-flex min-h-10 items-center gap-2 px-3"
        onClick={(event) => {
          event.currentTarget
            .closest('[role="dialog"]')
            ?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
          tour.openHub();
        }}
      >
        <CircleHelp aria-hidden="true" className="h-4 w-4" />
        {translate('tour.openHub')}
      </button>
    </fieldset>
  );
}
