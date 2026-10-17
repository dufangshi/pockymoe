import { useHostCapabilities } from '../lib/hostCapabilities';
import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
  type CSSProperties,
} from 'react';
import { createPortal } from 'react-dom';
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Columns2,
  Folder,
  Laptop,
  Loader2,
  MessageSquare,
  RotateCcw,
  Star,
  Users,
  X,
} from 'lucide-react';
import type { ThreadDto, WorkspaceDto } from '@pockymoe/shared';
import { StatusMark, navigationStatusLabel } from '@pockymoe/thread-ui';
import { translate as t, useI18n } from '@pockymoe/thread-ui/i18n';
import {
  rankSplitDevices,
  rankSplitThreads,
  rankSplitWorkspaces,
  splitThreadKey,
  useWorkbenchSplitCatalog,
  type CatalogResource,
  type SplitNavigationThread,
  type SplitThreadSelection,
} from '../pages/useWorkbenchSplitCatalog';

export type { SplitThreadSelection } from '../pages/useWorkbenchSplitCatalog';
export interface WorkbenchThreadPickerProps {
  deviceId: string | null;
  workspaceId: string;
  threadId: string;
  threads: ThreadDto[];
  navigationThreads?: SplitNavigationThread[];
  reference?: { deviceId: string | null; threadId: string } | null;
  splitActive?: boolean;
  onSelect: (selection: SplitThreadSelection) => void;
  onRestore?: () => void;
  onClose?: () => void;
  onCollaboration?: () => void;
  disabled?: boolean;
}
type Location = {
  deviceId: string | null;
  deviceName?: string | undefined;
  workspace?: WorkspaceDto;
} | null;

export function WorkbenchThreadPicker(props: WorkbenchThreadPickerProps) {
  useI18n();
  const { devicePeer } = useHostCapabilities();
  const {
    deviceId,
    workspaceId,
    threadId,
    threads,
    navigationThreads = [],
    reference,
    splitActive,
    onSelect,
  } = props;
  const [open, setOpen] = useState(false);
  const [location, setLocation] = useState<Location>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<CSSProperties>({
    top: 60,
    left: 12,
    maxHeight: 560,
  });
  // On a phone the picker is a sheet rising from the bottom over a scrim, like
  // the other secondary windows; elsewhere it hangs under its button.
  const [sheet, setSheet] = useState(false);
  const titleId = useId();
  const catalog = useWorkbenchSplitCatalog(open, deviceId, workspaceId);
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  useEffect(() => {
    setOpen(false);
    setLocation(null);
  }, [deviceId, workspaceId, threadId]);
  useLayoutEffect(() => {
    if (!open) return;
    const positionPanel = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      if (window.innerWidth < 640) {
        setSheet(true);
        setPosition({ left: 0, right: 0, bottom: 0, width: '100%', maxHeight: 'min(80dvh, 640px)' });
        return;
      }
      setSheet(false);
      const width = Math.min(372, window.innerWidth - 24);
      const top = Math.min(
        rect.bottom + 8,
        Math.max(12, window.innerHeight - 180),
      );
      setPosition({
        top,
        left: Math.max(
          12,
          Math.min(rect.right - width, window.innerWidth - width - 12),
        ),
        maxHeight: window.innerHeight - top - 12,
      });
    };
    positionPanel();
    window.addEventListener('resize', positionPanel);
    window.addEventListener('scroll', positionPanel, true);
    return () => {
      window.removeEventListener('resize', positionPanel);
      window.removeEventListener('scroll', positionPanel, true);
    };
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (
        !panel.current?.contains(event.target as Node) &&
        !trigger.current?.contains(event.target as Node)
      )
        setOpen(false);
    };
    const back = () => setOpen(false);
    document.addEventListener('pointerdown', outside);
    window.addEventListener('popstate', back);
    return () => {
      document.removeEventListener('pointerdown', outside);
      window.removeEventListener('popstate', back);
    };
  }, [open]);
  useEffect(() => {
    if (open)
      panel.current?.querySelector<HTMLButtonElement>('button')?.focus();
  }, [open, location]);
  const goBack = () =>
    setLocation(
      location?.workspace && location.deviceId !== deviceId
        ? { deviceId: location.deviceId, deviceName: location.deviceName }
        : null,
    );
  const choose = (thread: ThreadDto, targetDevice: string | null) => {
    onSelect({
      deviceId: targetDevice,
      workspaceId: thread.workspaceId,
      threadId: thread.id,
      title: thread.title,
    });
    close();
  };
  const resourceState = <T,>(
    resource: CatalogResource<T>,
    retry: () => void,
    emptyMessage: string,
  ) => (
    <>
      {resource.loading && (
        <div className="workbench-thread-picker-state" role="status">
          <Loader2 size={15} className="is-spinning" />
          {t('workbench.splitLoading')}
        </div>
      )}
      {resource.error && (
        <div className="workbench-thread-picker-error" role="alert">
          <span>{resource.error}</span>
          <button type="button" onClick={retry}>
            <RotateCcw size={14} />
            {t('workbench.splitRetry')}
          </button>
        </div>
      )}
      {resource.loaded && !resource.loading && !resource.items.length && (
        <p className="workbench-thread-picker-state">{emptyMessage}</p>
      )}
    </>
  );
  const threadRows = (
    items: ThreadDto[],
    targetDevice: string | null,
  ): ReactNode =>
    rankSplitThreads(
      items.filter(
        (thread) =>
          !thread.closedAt &&
          !(thread.id === threadId && targetDevice === deviceId),
      ),
      targetDevice,
      navigationThreads,
    ).map((thread) => {
      const key = splitThreadKey(targetDevice, thread.id);
      const favorite =
        thread.isPinned ||
        navigationThreads.some((item) => item.key === key && item.favorite);
      const selected =
        reference?.threadId === thread.id &&
        reference.deviceId === targetDevice;
      return (
        <button
          type="button"
          className="workbench-thread-picker-row"
          key={key}
          data-thread-id={thread.id}
          data-device-id={targetDevice ?? 'local'}
          data-workspace-id={thread.workspaceId}
          onClick={() => choose(thread, targetDevice)}
          aria-current={selected ? 'true' : undefined}
        >
          <span
            className={`workbench-thread-picker-row-icon ${favorite ? 'is-favorite' : ''}`}
          >
            {favorite ? <Star size={16} /> : <MessageSquare size={16} />}
          </span>
          <span className="workbench-thread-picker-row-label">
            <strong>{thread.title || t('workbench.splitUntitled')}</strong>
            <small>
              {/* The same status mark and words as the sidebar and tabs; the
                  navigation knows which finished threads are unread. */}
              <StatusMark status={pickerStatus(thread, navigationThreads.find((item) => item.key === key)?.status)} />
              {pickerLabel(thread, navigationThreads.find((item) => item.key === key)?.status, t)}
            </small>
          </span>
          {selected && (
            <Check size={16} className="workbench-thread-picker-check" />
          )}
        </button>
      );
    });
  const workspaceRows = (
    items: WorkspaceDto[],
    targetDevice: string | null,
    deviceName?: string,
  ) =>
    rankSplitWorkspaces(items).map((workspace) => (
      <button
        type="button"
        className="workbench-thread-picker-row"
        key={workspace.id}
        data-workspace-id={workspace.id}
        data-device-id={targetDevice ?? 'local'}
        onClick={() => {
          setLocation({ deviceId: targetDevice, deviceName, workspace });
          void catalog.loadThreads(targetDevice);
        }}
      >
        <span className="workbench-thread-picker-row-icon">
          <Folder size={17} />
        </span>
        <span className="workbench-thread-picker-row-label">
          <strong>{workspace.label}</strong>
          <small>{workspace.absPath}</small>
        </span>
        {workspace.isFavorite && (
          <Star size={13} className="workbench-thread-picker-favorite" />
        )}
        <ChevronRight size={15} />
      </button>
    ));
  const currentResource = catalog.threads(deviceId);
  const currentThreads = (
    currentResource.loaded ? currentResource.items : threads
  ).filter(
    (thread) =>
      thread.workspaceId === workspaceId &&
      thread.id !== threadId &&
      !thread.closedAt,
  );
  const localWorkspaces = catalog.workspaces(deviceId);
  const otherWorkspaces = localWorkspaces.items.filter(
    (workspace) => workspace.id !== workspaceId,
  );
  const title =
    location?.workspace?.label ??
    location?.deviceName ??
    t('workbench.splitSession');
  return (
    <span className="workbench-thread-picker-actions">
      <button
        type="button"
        ref={trigger}
        className={`workbench-thread-picker-trigger ${splitActive ? 'is-active' : ''}`}
        title={t('workbench.splitSession')}
        aria-label={t('workbench.splitSession')}
        aria-pressed={splitActive}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? titleId + '-panel' : undefined}
        disabled={props.disabled}
        onClick={() => {
          setLocation(null);
          setOpen((value) => !value);
        }}
        data-testid="workbench-split-trigger"
      >
        <Columns2 data-icon="split" size={16} />
      </button>
      {open &&
        createPortal(
          <>
          {sheet && <div className="workbench-thread-picker-scrim" aria-hidden="true" />}
          <div
            ref={panel}
            id={titleId + '-panel'}
            role="dialog"
            data-testid="workbench-thread-picker"
            aria-labelledby={titleId}
            className={`workbench-thread-picker-popover${sheet ? ' is-sheet' : ''}`}
            style={position}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                close();
              } else if (
                (event.key === 'ArrowLeft' || event.key === 'Backspace') &&
                location
              ) {
                event.preventDefault();
                goBack();
              } else if (
                ['ArrowDown', 'ArrowUp', 'Home', 'End', 'Tab'].includes(
                  event.key,
                )
              ) {
                const buttons = Array.from(
                  panel.current?.querySelectorAll<HTMLButtonElement>(
                    'button:not(:disabled)',
                  ) ?? [],
                );
                const index = buttons.indexOf(
                  document.activeElement as HTMLButtonElement,
                );
                if (!buttons.length) return;
                event.preventDefault();
                const next =
                  event.key === 'Home'
                    ? 0
                    : event.key === 'End'
                      ? buttons.length - 1
                      : (index +
                          (event.key === 'ArrowUp' ||
                          (event.key === 'Tab' && event.shiftKey)
                            ? -1
                            : 1) +
                          buttons.length) %
                        buttons.length;
                buttons[next]?.focus();
              }
            }}
          >
            <header className="workbench-thread-picker-heading">
              {location ? (
                <button
                  type="button"
                  aria-label={t('workbench.splitBack')}
                  onClick={goBack}
                >
                  <ArrowLeft size={17} />
                </button>
              ) : (
                <Columns2 size={18} />
              )}
              <div>
                <strong id={titleId}>{title}</strong>
                <small>
                  {location?.workspace
                    ? (location.deviceName ?? t('workbench.splitThisDevice'))
                    : t('workbench.splitChooseThread')}
                </small>
              </div>
              <button
                type="button"
                aria-label={t('workbench.splitClosePicker')}
                onClick={close}
              >
                <X size={16} />
              </button>
            </header>
            {splitActive && props.onClose && (
              <button
                type="button"
                className="workbench-thread-picker-row"
                onClick={() => { props.onClose?.(); close(); }}
              >
                <X size={16} /><span>{t('workbench.closeSplit')}</span>
              </button>
            )}
            {reference && !splitActive && props.onRestore && (
              <button
                type="button"
                className="workbench-thread-picker-row"
                onClick={() => { props.onRestore?.(); close(); }}
              >
                <RotateCcw data-icon="restore" size={16} /><span>{t('workbench.restoreSplit')}</span>
              </button>
            )}
            <div className="workbench-thread-picker-scroll">
              {!location ? (
                <>
                  <div className="workbench-thread-picker-section-label">
                    {t('workbench.splitCurrentWorkspace')}
                  </div>
                  {threadRows(currentThreads, deviceId)}
                  {resourceState(
                    {
                      ...currentResource,
                      items: currentThreads,
                      loaded:
                        currentResource.loaded || currentThreads.length > 0,
                    },
                    () => void catalog.loadThreads(deviceId, true),
                    t('workbench.splitNoThreads'),
                  )}
                  <div className="workbench-thread-picker-section-label">
                    {t('workbench.splitOtherWorkspaces')}
                  </div>
                  {workspaceRows(otherWorkspaces, deviceId)}
                  {resourceState(
                    { ...localWorkspaces, items: otherWorkspaces },
                    () => void catalog.loadWorkspaces(deviceId, true),
                    t('workbench.splitNoWorkspaces'),
                  )}
                  {devicePeer && deviceId && (
                    <>
                      <div className="workbench-thread-picker-section-label">
                        {t('workbench.splitOtherDevices')}
                      </div>
                      {rankSplitDevices(
                        catalog.devices.items,
                        navigationThreads,
                      ).map((device) => (
                        <button
                          type="button"
                          className="workbench-thread-picker-row"
                          key={device.id}
                          data-device-id={device.id}
                          onClick={() => {
                            setLocation({
                              deviceId: device.id,
                              deviceName: device.name,
                            });
                            void catalog.loadWorkspaces(device.id);
                          }}
                        >
                          <span className="workbench-thread-picker-row-icon">
                            <Laptop size={17} />
                          </span>
                          <span className="workbench-thread-picker-row-label">
                            <strong>{device.name}</strong>
                            <small>
                              <span
                                className={`workbench-split-status ${device.connected ? 'is-connected' : ''}`}
                              />
                              {t(
                                device.connected
                                  ? 'workbench.splitOnline'
                                  : 'workbench.splitOffline',
                              )}
                            </small>
                          </span>
                          <ChevronRight size={15} />
                        </button>
                      ))}
                      {resourceState(
                        catalog.devices,
                        () => void catalog.loadDevices(true),
                        t('workbench.splitNoDevices'),
                      )}
                    </>
                  )}
                </>
              ) : location.workspace ? (
                <>
                  {threadRows(
                    catalog
                      .threads(location.deviceId)
                      .items.filter(
                        (thread) =>
                          thread.workspaceId === location.workspace!.id,
                      ),
                    location.deviceId,
                  )}
                  {resourceState(
                    {
                      ...catalog.threads(location.deviceId),
                      items: catalog
                        .threads(location.deviceId)
                        .items.filter(
                          (thread) =>
                            thread.workspaceId === location.workspace!.id &&
                            !thread.closedAt &&
                            !(
                              location.deviceId === deviceId &&
                              thread.id === threadId
                            ),
                        ),
                    },
                    () => void catalog.loadThreads(location.deviceId, true),
                    t('workbench.splitNoThreads'),
                  )}
                </>
              ) : (
                <>
                  {workspaceRows(
                    catalog.workspaces(location.deviceId).items,
                    location.deviceId,
                    location.deviceName,
                  )}
                  {resourceState(
                    catalog.workspaces(location.deviceId),
                    () => void catalog.loadWorkspaces(location.deviceId, true),
                    t('workbench.splitNoWorkspaces'),
                  )}
                </>
              )}
            </div>
            {props.onCollaboration && (
              <footer>
                <button
                  type="button"
                  onClick={() => {
                    props.onCollaboration?.();
                    close();
                  }}
                >
                  <Users size={15} />
                  {t('workbench.collaboration')}
                  <ChevronRight size={14} />
                </button>
              </footer>
            )}
          </div>
          </>,
          document.body,
        )}
    </span>
  );
}

/** A thread's navigation status: the sidebar's when it knows the thread, else
    from the thread itself (running, failed, interrupted or idle). */
function pickerStatus(thread: ThreadDto, navigationStatus: string | undefined) {
  if (navigationStatus) return navigationStatus;
  if (thread.status === 'running' || thread.status === 'recovering') return 'running';
  if (thread.status === 'failed' || thread.status === 'interrupted') return thread.status;
  return 'idle';
}

/** Its words: the sidebar's when known; an idle thread the navigation has not
    seen is "Ready" (whether it was read is unknown). */
function pickerLabel(thread: ThreadDto, navigationStatus: string | undefined, t: (key: 'workbench.ready') => string) {
  const status = pickerStatus(thread, navigationStatus);
  return !navigationStatus && status === 'idle' ? t('workbench.ready') : navigationStatusLabel(status);
}
