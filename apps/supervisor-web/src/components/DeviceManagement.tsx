import './device-management.css';
import { sharingEditInput } from '../lib/signedSharing';
import { OrphanDeviceGrants } from '../components/SignedShareDetails';
import { requireUnlock } from '../lib/accountVault';
import { removeDeviceFromAccount } from '../lib/deviceRemoval';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import { DeviceSetupProgress } from './DeviceSetupProgress';
import {
  relaySupervisorCommand,
  validateDeviceSetupToken,
  type SupervisorPlatform,
  type DeviceBindingFlags,
} from '../lib/deviceSetupCommands';
import { DeviceProtectionBadge } from '../components/DeviceProtectionBadge';
import { useDeviceProtection } from '../lib/useDeviceProtection';
import { getTransportStatus } from '../lib/relayTransport';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n, type TranslationKey } from '@pockymoe/thread-ui/i18n';
import { Select, ErrorMark } from '@pockymoe/thread-ui';
import { SharedAccessPanel } from '../components/sharedAccess/SharedAccessPanel';
import { sharedItems, type ShareDirection, type SharedItem } from '../components/sharedAccess/model';
import { useShareAttention } from '../components/sharedAccess/useShareAttention';
import { SecurityVerification } from '../components/RelaySecurity';
import { securityRequest, type SecurityStatus } from '../lib/relaySecurity';
import { request } from '../lib/api';
import { DeviceEncryptionStatus } from '../components/DeviceEncryptionStatus';
import { AnchoredPopover } from '../components/AnchoredPopover';
import {
  Check,
  Copy,
  KeyRound,
  Link2Off,
  Pencil,
  MonitorSmartphone,
  Share2,
} from 'lucide-react';
import {
  FormEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { FormDialog } from './FormDialog';
import { RenameDialog } from './RenameDialog';
import { DeviceStateMark } from './DeviceStateMark';
import { IsolationMark } from './managed/IsolationMark';
import type { DeviceManageAction } from './managed/deviceActions';
import { useNavigate, useSearchParams } from 'react-router-dom';

import type {
  RelayCreateDeviceResultDto,
  RelayAccessGrantDto,
  RelayDeviceDto,
  RelayPortalSummaryDto,
  RelaySessionShareDto,
  RelayThreadAccessDto,
  RelayWorkspaceAccessDto,
} from '@pockymoe/shared';
import {
  ApiError,
  createRelayGrant,
  createRelayDevice,
  enableRelayMode,
  fetchAgentIsolation,
  fetchRelayPortal,
  revokeRelayGrant,
  revokeRelayShare,
  leaveRelayShare,
  leaveRelayGrant,
  setDeviceIsolation,
  setSelectedRelayDeviceId,
  setSelectedRelayThreadId,
  updateRelayGrant,
  updateRelayShare,
} from '../lib/api';
import { threadHref, threadsHref, workspacesHref } from '../lib/relayRoutes';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { useDevicePresence } from '../lib/useDevicePresence';
import { SHARING_CHANGED_EVENT, sharePendingHref } from '../lib/sharePending';
import { controlShares, SharedControlReasons, type ControlShare } from '../components/SharedControlReasons';

const RELAY_PORTAL_REFRESH_INTERVAL_MS = 3000;

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : fallback;
}

function verificationCancelled(caught: unknown) {
  return caught instanceof DOMException && caught.name === 'AbortError';
}

export function mergeRelayPortalSummary(
  previous: RelayPortalSummaryDto | null,
  next: RelayPortalSummaryDto,
): RelayPortalSummaryDto {
  if (!previous) {
    return sanitizeRelayPortalSummary(next);
  }

  return {
    ...next,
    sharedWithMe: mergeShareMetadata(previous.sharedWithMe, next.sharedWithMe),
    sharedByMe: mergeShareMetadata(previous.sharedByMe, next.sharedByMe),
    sharedDevicesWithMe: mergeGrantMetadata(
      previous.sharedDevicesWithMe ?? [],
      next.sharedDevicesWithMe ?? [],
    ),
    sharedThreadsWithMe: mergeGrantMetadata(
      previous.sharedThreadsWithMe ?? [],
      next.sharedThreadsWithMe ?? [],
    ),
    grantsByMe: mergeGrantMetadata(
      previous.grantsByMe ?? [],
      next.grantsByMe ?? [],
    ),
  };
}

function sanitizeRelayPortalSummary(
  summary: RelayPortalSummaryDto,
): RelayPortalSummaryDto {
  return {
    ...summary,
    sharedWithMe: summary.sharedWithMe.map(sanitizeShareMetadata),
    sharedByMe: summary.sharedByMe.map(sanitizeShareMetadata),
    sharedDevicesWithMe: (summary.sharedDevicesWithMe ?? []).map(
      sanitizeGrantMetadata,
    ),
    sharedThreadsWithMe: (summary.sharedThreadsWithMe ?? []).map(
      sanitizeGrantMetadata,
    ),
    grantsByMe: (summary.grantsByMe ?? []).map(sanitizeGrantMetadata),
  };
}

function mergeShareMetadata(
  previousShares: RelaySessionShareDto[],
  nextShares: RelaySessionShareDto[],
) {
  const previousById = new Map(
    previousShares.map((share) => [share.id, share]),
  );
  return nextShares.map((share) => {
    const previous = previousById.get(share.id);
    if (!previous) {
      return sanitizeShareMetadata(share);
    }
    const nextThreadTitle = stableShareThreadTitle(share);
    const previousThreadTitle = stableShareThreadTitle(previous);
    return {
      ...share,
      threadTitle: nextThreadTitle ?? previousThreadTitle,
      workspaceLabel: share.workspaceLabel ?? previous.workspaceLabel,
    };
  });
}

function sanitizeShareMetadata(
  share: RelaySessionShareDto,
): RelaySessionShareDto {
  return {
    ...share,
    threadTitle: stableShareThreadTitle(share),
  };
}

function mergeGrantMetadata(
  previousGrants: RelayAccessGrantDto[],
  nextGrants: RelayAccessGrantDto[],
) {
  const previousById = new Map(
    previousGrants.map((grant) => [grant.id, grant]),
  );
  return nextGrants.map((grant) => {
    const previous = previousById.get(grant.id);
    if (!previous) {
      return sanitizeGrantMetadata(grant);
    }
    const nextThreadTitle = stableGrantThreadTitle(grant);
    const previousThreadTitle = stableGrantThreadTitle(previous);
    return {
      ...grant,
      threadTitle: nextThreadTitle ?? previousThreadTitle,
      workspaceLabel: grant.workspaceLabel ?? previous.workspaceLabel,
      deviceName: grant.deviceName?.trim() || previous.deviceName,
    };
  });
}

function sanitizeGrantMetadata(
  grant: RelayAccessGrantDto,
): RelayAccessGrantDto {
  return {
    ...grant,
    threadTitle: stableGrantThreadTitle(grant),
  };
}

export type DeviceManagementAction = 'add' | DeviceManageAction;
/**
 * One capability implementation for the Web, desktop dialogs and device access section.
 * With `action` it shows only that action's window, in place, for `deviceId`: the
 * device card, the map and the rows here all open the same rename, share,
 * isolation, settings and removal windows (deviceActions).
 */
export function DeviceManagement({ deviceId, action, known, pane, sharingOnly = false, dialogsOnly = false, onClose, onRemoved, onRenamed, onRenameUnsupported }: { sharingOnly?: boolean; dialogsOnly?: boolean; onClose?: (() => void) | undefined; deviceId?: string | undefined; action?: DeviceManagementAction | undefined;
  /** A tab of the device's page for `deviceId`: its own settings (name, connection,
      isolation), or who it is shared with. */
  pane?: 'settings' | 'access' | undefined;
  /** What the page that asked already shows of the device, so its window opens at once
      (and focus returns to the control that opened it) instead of after the account is read again. */
  known?: { id: string; name: string; connected: boolean } | undefined;
  /** The device left the account: removed here, or found already removed. */
  onRemoved?: ((deviceId: string) => void) | undefined;
  /** The new name was saved. */
  onRenamed?: ((deviceId: string) => void) | undefined;
  /** This relay refuses renames (an older relay). */
  onRenameUnsupported?: (() => void) | undefined }) {
  const { locale: i18nLocale } = useI18n();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [rawPortal, setPortal] = useState<RelayPortalSummaryDto | null>(null);
  const presentPortal = useDevicePresence(rawPortal);
  const portal = useMemo(() => !deviceId || !presentPortal ? presentPortal : {
    ...presentPortal,
    devices: presentPortal.devices.filter(device => device.id === deviceId),
    sharedWithMe: presentPortal.sharedWithMe.filter(row => row.deviceId === deviceId),
    sharedByMe: presentPortal.sharedByMe.filter(row => row.deviceId === deviceId),
    grantsByMe: presentPortal.grantsByMe?.filter(row => row.deviceId === deviceId),
    sharedDevicesWithMe: presentPortal.sharedDevicesWithMe?.filter(row => row.deviceId === deviceId),
    sharedThreadsWithMe: presentPortal.sharedThreadsWithMe?.filter(row => row.deviceId === deviceId),
  }, [deviceId, presentPortal]);
  const [rotatingDevice, setRotatingDevice] = useState<RelayDeviceDto | null>(null);
  const [rotationVerification,setRotationVerification] = useState<SecurityStatus | null>(null);
  const [setupVerification, setSetupVerification] = useState<SecurityStatus | null>(null);
  const mountedRef = useRef(true);
  const pendingVerificationRef = useRef<{
    promise: Promise<void>;
    resolve: () => void;
    reject: (error: unknown) => void;
  } | null>(null);

  function finishSetupVerification(verified: boolean) {
    const pending = pendingVerificationRef.current;
    pendingVerificationRef.current = null;
    setSetupVerification(null);
    if (verified) pending?.resolve();
    else pending?.reject(new DOMException('Aborted', 'AbortError'));
  }

  function reverifySetup() {
    if (pendingVerificationRef.current) {
      return pendingVerificationRef.current.promise;
    }
    let resolve!: () => void;
    let reject!: (error: unknown) => void;
    const promise = new Promise<void>((accept, decline) => {
      resolve = accept;
      reject = decline;
    });
    const pending = { promise, resolve, reject };
    pendingVerificationRef.current = pending;
    void securityRequest<SecurityStatus>('').then(
      (status) => {
        if (pendingVerificationRef.current === pending) setSetupVerification(status);
      },
      (error: unknown) => {
        if (pendingVerificationRef.current === pending) {
          pendingVerificationRef.current = null;
          pending.reject(error);
        }
      },
    );
    return promise;
  }

  async function withSetupReverification<T>(action: () => Promise<T>): Promise<T> {
    try {
      return await action();
    } catch (caught) {
      if (
        !(caught instanceof ApiError) || caught.statusCode !== 403 ||
        caught.payload.code !== 'reauthentication_required'
      ) throw caught;
      if (!mountedRef.current) throw new DOMException('Aborted', 'AbortError');
      // Keep the original action (and clipboard promise) pending through the
      // same security verification used for token replacement. Retry only once.
      await reverifySetup();
      if (!mountedRef.current) throw new DOMException('Aborted', 'AbortError');
      return action();
    }
  }

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      pendingVerificationRef.current?.reject(new DOMException('Aborted', 'AbortError'));
      pendingVerificationRef.current = null;
    };
  }, []);
  async function rotateDevice(verified=false) {
    if (!rotatingDevice) return;
    try {
      if (!verified) {const status=await securityRequest<SecurityStatus>(''); if (!status.recentlyVerified) {setRotationVerification(status);return;}}
      const binding = await prepareDeviceEnrollment(rotatingDevice.id);
      setCreatedBinding(binding);
      const result=await request<RelayCreateDeviceResultDto>(`relay/devices/${rotatingDevice.id}/token`,{method:'POST'});
      validateDeviceSetupToken(result.token);
      // The new token and its commands show in the settings window that asked for it.
      setCreatedDevice(result);setRotatingDevice(null);setRotationVerification(null);await load({showLoading:false});
    } catch(e) {
      // Back to the Replace token window, whose button carries the mark.
      setRotationVerification(null);
      setError({ at: 'rotate', message: errorMessage(e,translate("devices.tokenRotationFailed")) });
    }
  }
  const [deviceName, setDeviceName] = useState('');
  const [createdBinding, setCreatedBinding] = useState<DeviceBindingFlags | null>(null);
  const [createdDevice, setCreatedDevice] =
    useState<RelayCreateDeviceResultDto | null>(null);
  const [copiedDeviceId, setCopiedDeviceId] = useState<string | null>(null);
  const [deviceCopyError, setDeviceCopyError] = useState<{
    deviceId: string;
    message: string;
  } | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  // Why the first load or an action failed, as a mark after its control
  // (`at`: 'load', 'create', 'remove', 'revoke', 'rotate'). A failed
  // background refresh keeps the last list silently.
  const [error, setError] = useState<{ at: 'load' | 'create' | 'remove' | 'revoke' | 'rotate' | 'leave'; message: string } | null>(null);
  const [leaveTarget, setLeaveTarget] = useState<SharedItem | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  // The page's tab and the shares' direction live in the address, so a link
  // or a reload returns to them.
  const shareDirection: ShareDirection = searchParams.get('dir') === 'out' ? 'out' : 'in';
  // On the Sharing tab of the devices page the tab is in the address too; a
  // device page's Access tab keeps its own tab and changes only the direction.
  const setShareDirection = (direction: ShareDirection) => {
    const next = new URLSearchParams(searchParams);
    if (!pane) next.set('tab', 'shared');
    if (direction === 'out') next.set('dir', 'out'); else next.delete('dir');
    setSearchParams(next, { replace: true });
  };
  const [editingShare, setEditingShare] = useState<RelaySessionShareDto | null>(
    null,
  );
  const [editingGrant, setEditingGrant] = useState<RelayAccessGrantDto | null>(
    null,
  );
  const [revokeTarget, setRevokeTarget] = useState<
    | { kind: 'grant'; grant: RelayAccessGrantDto }
    | { kind: 'share'; share: RelaySessionShareDto }
    | null
  >(null);
  const [deletingDevice, setDeletingDevice] =
    useState<RelayDeviceDto | null>(null);
  const [sharingDevice, setSharingDevice] = useState<RelayDeviceDto | null>(
    null,
  );
  const [addDeviceOpen, setAddDeviceOpen] = useState(action === 'add');
  const [isolatingDevice, setIsolatingDevice] = useState<RelayDeviceDto | null>(null);
  const [renamingDevice, setRenamingDevice] = useState<RelayDeviceDto | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const [renameBusy, setRenameBusy] = useState(false);
  // A key where the words are ours, so they follow a change of language; the relay's own message otherwise.
  const [renameError, setRenameError] = useState<{ key: TranslationKey } | { message: string } | null>(null);
  const [renameSupported, setRenameSupported] = useState(true);
  /** Every surface opens a device's management windows through here, in place. */
  function openAction(device: RelayDeviceDto, next: DeviceManageAction) {
    setError(null);
    setDialogError(null);
    if (next === 'rename') { setRenameDraft(device.name); setRenameError(null); setRenamingDevice(device); }
    if (next === 'share') setSharingDevice(device);
    if (next === 'isolate') setIsolatingDevice(device);
    if (next === 'remove') setDeletingDevice(device);
  }
  // Until the account answers, the window shows what its page already knows of the device.
  const knownDevice = useMemo<RelayDeviceDto | null>(() => known && known.id === deviceId && action && action !== 'add'
    ? { id: known.id, ownerUserId: '', name: known.name, token: null, tokenPreview: '', connected: known.connected, connectedAt: null, lastHeartbeatAt: null, createdAt: '' }
    : null, [action, deviceId, known?.id, known?.name, known?.connected]);
  /** The device as the account last answered it, for a window opened from an earlier answer. */
  const current = (device: RelayDeviceDto) => portal?.devices.find(entry => entry.id === device.id) ?? device;
  const openedAction = useRef(false);
  useEffect(() => {
    const device = portal?.devices.find(device => device.id === deviceId) ?? knownDevice;
    if (openedAction.current || !device || !action || action === 'add') return;
    openedAction.current = true;
    openAction(device, action);
  }, [portal, deviceId, action, knownDevice]);
  const hasLoadedPortalRef = useRef(false);
  const handledShareDeviceRequestRef = useRef<string | null>(null);
  const copiedResetTimeoutRef = useRef<number | null>(null);

  const load = useCallback(
    async (options?: { showLoading?: boolean; clearError?: boolean }) => {
      const showLoading = options?.showLoading ?? true;
      const clearError = options?.clearError ?? true;

      if (showLoading) {
        setLoading(true);
      }
      if (clearError) {
        setError(null);
      }
      try {
        enableRelayMode();
        const nextPortal = await fetchRelayPortal();
        const recoveredInitialLoad = !hasLoadedPortalRef.current;
        hasLoadedPortalRef.current = true;
        setPortal((current) => mergeRelayPortalSummary(current, nextPortal));
        if (recoveredInitialLoad) {
          setError(null);
        }
        return true;
      } catch (caught) {
        const message = errorMessage(caught, translate("devices.unableToLoadDevices"));
        if (showLoading || !hasLoadedPortalRef.current) {
          setError({ at: 'load', message });
        }
        return false;
      } finally {
        if (showLoading) {
          setLoading(false);
        }
      }
    },
    [],
  );

  // An approval or a signed grant shows at once, not at the next refresh.
  useEffect(() => {
    const reload = () => void load({ showLoading: false, clearError: false });
    window.addEventListener(SHARING_CHANGED_EVENT, reload);
    return () => window.removeEventListener(SHARING_CHANGED_EVENT, reload);
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    let refreshing = false;
    let retryCount = 0;
    let timeoutId: number | null = null;

    function scheduleNextRefresh() {
      if (cancelled || document.visibilityState === 'hidden') {
        return;
      }
      const delay = Math.min(
        RELAY_PORTAL_REFRESH_INTERVAL_MS * 2 ** retryCount,
        48_000,
      );
      timeoutId = window.setTimeout(() => {
        timeoutId = null;
        void refresh(false);
      }, delay);
    }

    async function refresh(initial: boolean) {
      if (refreshing) {
        return;
      }
      refreshing = true;
      try {
        const succeeded = await load(
          initial
            ? undefined
            : { showLoading: false, clearError: false },
        );
        if (cancelled) {
          return;
        }
        retryCount = succeeded ? 0 : Math.min(retryCount + 1, 4);
        scheduleNextRefresh();
      } finally {
        refreshing = false;
      }
    }

    function handleVisibilityChange() {
      if (timeoutId !== null) {
        window.clearTimeout(timeoutId);
        timeoutId = null;
      }
      if (document.visibilityState !== 'hidden') {
        void refresh(false);
      }
    }

    void refresh(true);
    document.addEventListener('visibilitychange', handleVisibilityChange);

    return () => {
      cancelled = true;
      if (timeoutId !== null) {
        window.clearTimeout(timeoutId);
      }
      document.removeEventListener(
        'visibilitychange',
        handleVisibilityChange,
      );
    };
  }, [load]);

  useEffect(() => () => {
    if (copiedResetTimeoutRef.current !== null) {
      window.clearTimeout(copiedResetTimeoutRef.current);
    }
  }, []);

  useEffect(() => {
    const requestedDeviceId = searchParams.get('shareDevice');
    if (
      !requestedDeviceId ||
      !portal ||
      handledShareDeviceRequestRef.current === requestedDeviceId
    ) {
      return;
    }

    const device = portal.devices.find(
      (entry) => entry.id === requestedDeviceId,
    );
    if (!device) {
      return;
    }

    handledShareDeviceRequestRef.current = requestedDeviceId;
    setDialogError(null);
    setSharingDevice(device);
    const nextParams = new URLSearchParams(searchParams);
    nextParams.delete('shareDevice');
    setSearchParams(nextParams, { replace: true });
  }, [portal, searchParams, setSearchParams]);

  function closeRename() {
    setRenamingDevice(null);
    setRenameError(null);
    if (action === 'rename') onClose?.();
  }

  async function renameDevice() {
    const device = renamingDevice;
    const name = renameDraft.trim();
    if (!device || !name || renameBusy) return;
    if (name === device.name) { closeRename(); return; }
    const hasControl = [...name].some(character => { const code = character.codePointAt(0)!; return code < 32 || (code >= 127 && code <= 159); });
    if ([...name].length > 64 || hasControl) { setRenameError({ key: 'desktopDevices.invalidName' }); return; }
    setRenameBusy(true);
    setRenameError(null);
    try {
      await request(`relay/devices/${encodeURIComponent(device.id)}`, { method: 'PATCH', body: JSON.stringify({ name }) });
      await load({ showLoading: false });
      onRenamed?.(device.id);
      closeRename();
    } catch (caught) {
      if (caught instanceof ApiError && [404, 405].includes(caught.statusCode)) {
        // An older relay: say so here, and grey the action out from now on.
        setRenameSupported(false);
        onRenameUnsupported?.();
        setRenameError({ key: 'desktopDevices.renameUnsupported' });
      } else setRenameError(caught instanceof Error && caught.message ? { message: caught.message } : { key: 'desktopDevices.renameFailed' });
    } finally {
      setRenameBusy(false);
    }
  }

  async function addDevice(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy('create');
    setError(null);
    try {
      await requireUnlock('bind-device');
      const result = await withSetupReverification(() =>
        createRelayDevice({ name: deviceName }),
      );
      validateDeviceSetupToken(result.token);
      setCreatedBinding(await prepareDeviceEnrollment(result.device.id));
      setCreatedDevice(result);
      setDeviceName('');
      setAddDeviceOpen(false);
      await load({ showLoading: false });
    } catch (caught) {
      if (!verificationCancelled(caught)) {
        setError({ at: 'create', message: errorMessage(caught, translate("devices.unableToCreateDevice")) });
      }
    } finally {
      setBusy(null);
    }
  }

  async function removeDevice() {
    const device = deletingDevice;
    if (!device) {
      return;
    }
    setBusy(device.id);
    setError(null);
    try {
      await removeDeviceFromAccount(device.id);
      if (createdDevice?.device.id === device.id) {
        setCreatedDevice(null);
      }
      const reloaded = await load({ showLoading: false });
      if (!reloaded) {
        setPortal(current => current ? {
          ...current,
          devices: current.devices.filter(entry => entry.id !== device.id),
        } : current);
      }
      setDeletingDevice(null); if (action === 'remove') onClose?.();
      onRemoved?.(device.id);
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => {
          if (document.activeElement === document.body) {
            document.getElementById('devices-heading')?.focus();
          }
        });
      });
    } catch (caught) {
      setError({ at: 'remove', message: errorMessage(caught, translate("devices.unableToDeleteDevice")) });
    } finally {
      setBusy(null);
    }
  }

  function openSharedSession(share: RelaySessionShareDto) {
    if (share.deviceConnected !== true) return;
    setSelectedRelayDeviceId(share.deviceId);
    setSelectedRelayThreadId(share.threadId);
    navigate(threadHref(share.threadId, share.deviceId));
  }

  function openSharedGrant(grant: RelayAccessGrantDto) {
    if (grant.deviceConnected !== true) return;
    setSelectedRelayDeviceId(grant.deviceId);
    setSelectedRelayThreadId(grant.threadId);
    if (grant.threadId) {
      navigate(threadHref(grant.threadId, grant.deviceId));
      return;
    }
    // A shared workspace opens on its own threads; the device's list is not shared.
    if (grant.scope === 'workspace' && grant.workspaceId) {
      navigate(threadsHref(grant.workspaceId, grant.deviceId));
      return;
    }
    navigate(workspacesHref(grant.deviceId));
  }

  async function createDeviceGrant(
    device: RelayDeviceDto,
    input: {
      targetIdentifier: string;
      label: string | null;
      threadAccess: RelayThreadAccessDto;
      workspaceAccess: RelayWorkspaceAccessDto;
      canCreateThreads: boolean;
    },
  ) {
    setBusy(`grant:create:${device.id}`);
    setDialogError(null);
    try {
      await createRelayGrant({
        ...input,
        deviceId: device.id,
        scope: 'device',
        workspaceScope: 'all',
        workspaceIds: [],
      });
      setSharingDevice(null); if (action === 'share') onClose?.();
      setDialogError(null);
      await load({ showLoading: false });
    } catch (caught) {
      setDialogError(errorMessage(caught, translate("devices.unableToShareDevice")));
    } finally {
      setBusy(null);
    }
  }

  async function updateAccessGrant(
    grant: RelayAccessGrantDto,
    input: {
      label: string | null;
      threadAccess: RelayThreadAccessDto;
      workspaceAccess: RelayWorkspaceAccessDto;
      canCreateThreads: boolean;
      expiresAt: string | null;
    },
  ) {
    setBusy(`grant:${grant.id}`);
    setDialogError(null);
    try {
      await updateRelayGrant(grant.id, {
        ...input,
        workspaceId: grant.workspaceId,
        workspaceScope: grant.workspaceScope,
        workspaceIds: grant.workspaceIds,
      }, sharingEditInput('grant', grant));
      setEditingGrant(null);
      setDialogError(null);
      await load({ showLoading: false });
    } catch (caught) {
      setDialogError(
        errorMessage(caught, translate("devices.unableToUpdateSharedAccess")),
      );
    } finally {
      setBusy(null);
    }
  }

  async function revokeAccessGrant(grant: RelayAccessGrantDto) {
    setBusy(`grant:${grant.id}`);
    setError(null);
    try {
      await revokeRelayGrant(grant.id);
      setRevokeTarget(null);
      await load({ showLoading: false });
    } catch (caught) {
      setError({ at: 'revoke', message: errorMessage(caught, translate("devices.unableToRemoveSharedAccess")) });
    } finally {
      setBusy(null);
    }
  }

  async function updateSharedSession(
    share: RelaySessionShareDto,
    input: {
      label: string | null;
      threadAccess: RelayThreadAccessDto;
      workspaceAccess: RelayWorkspaceAccessDto;
      expiresAt: string | null;
    },
  ) {
    setBusy(`share:${share.id}`);
    setDialogError(null);
    try {
      await updateRelayShare(share.id, {
        ...input,
        workspaceId: share.workspaceId,
      }, sharingEditInput('share', share));
      setEditingShare(null);
      setDialogError(null);
      await load({ showLoading: false });
    } catch (caught) {
      setDialogError(
        errorMessage(caught, translate("devices.unableToUpdateSharedThread")),
      );
    } finally {
      setBusy(null);
    }
  }

  async function leaveSharedAccess(item: SharedItem) {
    setBusy(item.key);
    setError(null);
    try {
      if (item.source.kind === 'share') await leaveRelayShare(item.source.row.id);
      else await leaveRelayGrant(item.source.row.id);
      setLeaveTarget(null);
      await load({ showLoading: false });
    } catch (caught) {
      setError({ at: 'leave', message: errorMessage(caught, translate('sharing.unableToLeave')) });
    } finally {
      setBusy(null);
    }
  }

  async function revokeSharedSession(share: RelaySessionShareDto) {
    setBusy(`share:${share.id}`);
    setError(null);
    try {
      await revokeRelayShare(share.id);
      setRevokeTarget(null);
      await load({ showLoading: false });
    } catch (caught) {
      setError({ at: 'revoke', message: errorMessage(caught, translate("devices.unableToRemoveSharedThreadAccess")) });
    } finally {
      setBusy(null);
    }
  }

  async function copySupervisorSetup(
    device: RelayDeviceDto,
    platform: SupervisorPlatform,
  ) {
    const clipboard = navigator.clipboard;
    if (!clipboard || typeof clipboard.writeText !== 'function') {
      setCopiedDeviceId(null);
      setDeviceCopyError({
        deviceId: device.id,
        message:
          translate("devices.clipboardAccessIsUnavailableCheckBrowserPermissions"),
      });
      return;
    }

    try {
      const command = prepareDeviceEnrollment(device.id).then(async binding => {
        const { token } = await withSetupReverification(() =>
          request<{ token: string }>(`relay/devices/${device.id}/setup-token`, { method: 'POST' }),
        );
        return relaySupervisorCommand(token, platform, binding);
      });
      // Start the clipboard operation within the tap gesture, including Safari.
      if (typeof ClipboardItem !== 'undefined' && typeof clipboard.write === 'function') {
        await clipboard.write([new ClipboardItem({
          'text/plain': command.then((text) => new Blob([text], { type: 'text/plain' })),
        })]);
      } else {
        await clipboard.writeText(await command);
      }
      setDeviceCopyError(null);
      setCopiedDeviceId(device.id);
      if (copiedResetTimeoutRef.current !== null) {
        window.clearTimeout(copiedResetTimeoutRef.current);
      }
      copiedResetTimeoutRef.current = window.setTimeout(() => {
        setCopiedDeviceId((current) =>
          current === device.id ? null : current,
        );
        copiedResetTimeoutRef.current = null;
      }, 1600);
    } catch (caught) {
      setCopiedDeviceId(null);
      if (verificationCancelled(caught)) return;
      setDeviceCopyError({
        deviceId: device.id,
        message:
          translate("devices.unableToCopyTheSetupCommandCheck"),
      });
    }
  }

  const outgoingItems = useMemo(() => sharedItems(portal, 'out'), [portal]);
  const { attention, refresh: refreshAttention } = useShareAttention(outgoingItems);
  const shareActions = {
    onOpen: (item: SharedItem) => item.source.kind === 'share' ? openSharedSession(item.source.row) : openSharedGrant(item.source.row),
    onEdit: (item: SharedItem) => {
      setDialogError(null);
      if (item.source.kind === 'share') setEditingShare(item.source.row);
      else setEditingGrant(item.source.row);
    },
    onRevoke: (item: SharedItem) => {
      setError(null);
      setRevokeTarget(item.source.kind === 'share' ? { kind: 'share', share: item.source.row } : { kind: 'grant', grant: item.source.row });
    },
    onLeave: (item: SharedItem) => {
      setError(null);
      setLeaveTarget(item);
    },
    onShowCode: (item: SharedItem) => navigate(sharePendingHref({ kind: item.source.kind, id: item.source.row.id })),
  };
  const closeAdd = useManagementClose(Boolean(deviceName), () => setAddDeviceOpen(false));
  // A changed name is asked about before it is dropped (§4).
  // (Not when this relay cannot rename at all: there is nothing to lose.)
  const renameClose = useManagementClose(Boolean(renamingDevice && renameSupported && renameDraft !== renamingDevice.name), closeRename);
  const closeManagement = useManagementClose(action === 'add' && Boolean(deviceName), () => onClose?.());
  const paneDevice = pane && deviceId ? portal?.devices.find(entry => entry.id === deviceId) ?? null : null;
  const accessPanel = (
    <SharedAccessPanel
      portal={portal}
      loading={loading}
      direction={shareDirection}
      onDirectionChange={setShareDirection}
      attention={attention}
      onAttentionRefresh={refreshAttention}
      busyKey={busy}
      actions={shareActions}
      outgoingNotice={portal ? <><OrphanDeviceGrants portal={portal} kind="share" /><OrphanDeviceGrants portal={portal} kind="grant" /></> : null}
    />
  );

  const content = (
    <div>
      <div hidden={dialogsOnly || Boolean(action && action !== 'add')} className="space-y-6 device-management-embedded">
        {action === 'add' ? <>
          {addDeviceOpen ? (
            <AddDeviceForm
              busy={busy === 'create'}
              deviceName={deviceName}
              onChangeDeviceName={setDeviceName}
              error={error?.at === 'create' ? error.message : null}
              onClose={closeAdd.requestClose}
              onSubmit={addDevice}
            />
          ) : null}
          {createdDevice && createdBinding ? <DeviceTokenPanel result={createdDevice} binding={createdBinding} onOpen={onClose} /> : null}
        </> : pane === 'access' ? (
          // The device page's Access tab: who it is shared with, and what is shared from it.
          <section aria-label={translate('desktopDevices.access')}>{accessPanel}</section>
        ) : pane === 'settings' ? (
          paneDevice ? (
            <DeviceGeneralSettings
              device={paneDevice}
              controlShares={controlShares(portal, paneDevice.id)}
              renameSupported={renameSupported}
              onRename={() => openAction(paneDevice, 'rename')}
              copied={copiedDeviceId === paneDevice.id}
              copyError={deviceCopyError?.deviceId === paneDevice.id ? deviceCopyError.message : null}
              onCopySetup={(platform) => void copySupervisorSetup(paneDevice, platform)}
              onRotate={() => { setError(null); setRotatingDevice(paneDevice); }}
            >
              {createdDevice && createdBinding && createdDevice.device.id === paneDevice.id ? <DeviceTokenPanel result={createdDevice} binding={createdBinding} /> : null}
              <ConfirmDialog open={Boolean(rotatingDevice) && !rotationVerification} title={translate("devices.replaceDeviceToken")} description={translate("devices.theCurrentConnectionWillCloseUpdateThe")} confirmLabel={translate("devices.replaceToken")} error={rotatingDevice && error?.at === 'rotate' ? error.message : null} onConfirm={() => { setError(null); void rotateDevice(); }} onCancel={() => { setRotatingDevice(null); setError(null); }} />
            </DeviceGeneralSettings>
          ) : loading ? <LoadingRows label={translate("devices.loadingDevices")} />
            : <div className="product-empty flex items-center justify-center gap-1.5">{translate("devices.unavailable")}<ErrorMark message={error?.message ?? translate('desktopDevices.deviceMissing')} onRetry={() => void load()} /></div>
        ) : sharingOnly ? (
          <section role="tabpanel" id="devices-page-panel-shared" aria-labelledby="devices-page-tab-shared">{accessPanel}</section>
        ) : null}
      </div>
      {isolatingDevice ? <DeviceIsolationDialog device={current(isolatingDevice)} controlShares={controlShares(portal, isolatingDevice.id)} onClose={() => { setIsolatingDevice(null); if (action === 'isolate') onClose?.(); }} /> : null}
      <RenameDialog
        open={renamingDevice !== null}
        title={translate('desktopDevices.renameDevice')}
        label={translate('desktopDevices.deviceName')}
        value={renameDraft}
        busy={renameBusy}
        error={renameError ? 'key' in renameError ? translate(renameError.key) : renameError.message : null}
        onChange={(value) => { setRenameDraft(value); setRenameError(null); }}
        onCancel={() => { if (!renameBusy) renameClose.requestClose(); }}
        onSubmit={renameDevice}
      />
      {renameClose.confirmation}
      {editingShare ? (
        <SharePermissionsDialog
          busy={busy === `share:${editingShare.id}`}
          error={dialogError}
          share={editingShare}
          onClose={() => {
            setEditingShare(null);
            setDialogError(null);
          }}
          onSave={(input) => void updateSharedSession(editingShare, input)}
        />
      ) : null}
      {editingGrant ? (
        <GrantPermissionsDialog
          busy={busy === `grant:${editingGrant.id}`}
          error={dialogError}
          grant={editingGrant}
          onClose={() => {
            setEditingGrant(null);
            setDialogError(null);
          }}
          onSave={(input) => void updateAccessGrant(editingGrant, input)}
        />
      ) : null}
      {sharingDevice ? (
        <ShareDeviceDialog
          busy={busy === `grant:create:${sharingDevice.id}`}
          device={sharingDevice}
          error={dialogError}
          onClose={() => {
            setSharingDevice(null); if (action === 'share') onClose?.();
            setDialogError(null);
          }}
          onShare={(input) => void createDeviceGrant(sharingDevice, input)}
        />
      ) : null}
      {rotationVerification && <SecurityVerification status={rotationVerification} onCancel={()=> {setRotationVerification(null);setRotatingDevice(null);}} onVerified={()=>rotateDevice(true)} />}
      {setupVerification && (
        <SecurityVerification
          status={setupVerification}
          onCancel={() => finishSetupVerification(false)}
          onVerified={async () => finishSetupVerification(true)}
        />
      )}
      <ConfirmDialog
        open={deletingDevice !== null}
        title={translate("devices.removeDevice")}
        description={
          deletingDevice
            ? translate("devices.removeDeviceHelp", { value1: deletingDevice.name })
            : ''
        }
        confirmLabel={translate("devices.removeDevice")}
        busyLabel={translate("devices.removing")}
        busy={Boolean(deletingDevice && busy === deletingDevice.id)}
        error={deletingDevice && error?.at === 'remove' ? error.message : null}
        onCancel={() => {
          if (!deletingDevice || busy !== deletingDevice.id) {
            setDeletingDevice(null); if (action === 'remove') onClose?.();
            setError(null);
          }
        }}
        onConfirm={removeDevice}
      />
      <ConfirmDialog
        open={revokeTarget !== null}
        title={
          revokeTarget?.kind === 'grant'
            ? translate("devices.revokeSharedDeviceAccess")
            : translate("devices.revokeSharedThreadAccess")
        }
        description={revokeTarget ? revokeDescription(revokeTarget) : ''}
        confirmLabel={translate("devices.revokeAccess")}
        busyLabel={translate("devices.revoking")}
        busy={revokeTargetBusy(revokeTarget, busy)}
        error={revokeTarget && error?.at === 'revoke' ? error.message : null}
        onCancel={() => {
          if (!revokeTargetBusy(revokeTarget, busy)) {
            setRevokeTarget(null);
            setError(null);
          }
        }}
        onConfirm={() => {
          if (!revokeTarget) {
            return;
          }
          return revokeTarget.kind === 'grant'
            ? revokeAccessGrant(revokeTarget.grant)
            : revokeSharedSession(revokeTarget.share);
        }}
      />
      <ConfirmDialog
        open={leaveTarget !== null}
        title={translate('sharing.leaveTitle')}
        description={leaveTarget ? translate('sharing.leaveDescription', {
          scope: translate(leaveTarget.scope === 'device' ? 'sharing.kindDevice' : leaveTarget.scope === 'workspace' ? 'sharing.kindWorkspace' : 'sharing.kindThread'),
          name: leaveTarget.title,
          owner: leaveTarget.personName,
        }) : ''}
        confirmLabel={translate('sharing.leaveAccess')}
        busyLabel={translate('sharing.leaving')}
        busy={Boolean(leaveTarget && busy === leaveTarget.key)}
        error={leaveTarget && error?.at === 'leave' ? error.message : null}
        onCancel={() => {
          if (!leaveTarget || busy !== leaveTarget.key) {
            setLeaveTarget(null);
            setError(null);
          }
        }}
        onConfirm={() => (leaveTarget ? leaveSharedAccess(leaveTarget) : undefined)}
      />
    </div>
  );
  if (action && action !== 'add' && (portal ? !portal.devices.length : !knownDevice)) {
    // The list loaded and the account does not have this device: it was removed
    // elsewhere. Say so and let it leave the page, instead of an error to retry.
    const gone = Boolean(portal && deviceId && !loading && !error);
    const close = () => { if (gone && deviceId) onRemoved?.(deviceId); onClose?.(); };
    return <FormDialog className="device-management-dialog" title={translate('desktopDevices.deviceDetails')} onClose={close}>{loading ? <div role="status" className="product-skeleton h-24" aria-label={translate('devices.loadingDevices')} />
      : gone ? <div className="grid gap-4"><p className="text-sm text-[var(--theme-fg)]">{translate('devices.alreadyRemoved')}</p><div className="flex justify-end"><button className="relay-button-primary min-h-11 px-4 py-2" onClick={close} type="button">{translate('workbench.close')}</button></div></div>
      : <ErrorMark message={error?.message ?? translate('desktopDevices.deviceMissing')} onRetry={() => { void load(); }} />}</FormDialog>;
  }
  return action === 'add' && onClose ? <FormDialog className="device-management-dialog" title={translate('devices.addDevice')} busy={Boolean(busy)} onClose={closeManagement.requestClose}>{content}{closeAdd.confirmation}{closeManagement.confirmation}</FormDialog> : <>{content}{closeAdd.confirmation}</>;
}

export function AddDeviceForm({
  busy,
  deviceName,
  error,
  onChangeDeviceName,
  onClose,
  onSubmit,
}: {
  busy: boolean;
  deviceName: string;
  error: string | null;
  onChangeDeviceName: (value: string) => void;
  onClose: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <form
      aria-labelledby="add-device-heading"
      className="mt-4 border-y border-[var(--theme-border)] bg-[var(--theme-surface)] px-3 py-4 sm:px-4"
      id="add-device-form"
      onSubmit={onSubmit}
    >
      <h3
        className="text-sm font-semibold text-[var(--theme-fg)]"
        id="add-device-heading"
      >
        {translate("devices.createADeviceToken")}</h3>
      <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
        {translate("devices.nameThePrivateSupervisorThatWillUse")}</p>
      <div className="mt-4 flex flex-col gap-3 sm:flex-row sm:items-end">
        <label className="block min-w-0 flex-1 text-sm text-[var(--theme-fg-soft)]">
          {translate("devices.deviceName")}<input
            autoFocus
            className="relay-input mt-2 min-h-11 w-full"
            disabled={busy}
            onChange={(event) => onChangeDeviceName(event.target.value)}
            placeholder="MacBook Pro"
            required
            value={deviceName}
          />
        </label>
        <div className="flex shrink-0 items-center gap-2">
          <button
            className="relay-button-secondary min-h-11"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            {translate("devices.cancel")}</button>
          <button
            className="relay-button-primary inline-flex min-h-11 items-center gap-2"
            disabled={busy || !deviceName.trim()}
            type="submit"
          >
            <MonitorSmartphone className="h-4 w-4" />
            {busy ? translate("devices.creating_28ea76") : translate("devices.createToken")}
          </button>
          {error ? <ErrorMark message={error} /> : null}
        </div>
      </div>
    </form>
  );
}

function LoadingRows({ label }: { label: string }) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div aria-busy="true" aria-live="polite" className="divide-y divide-[var(--theme-border)]" role="status">
      <span className="sr-only">{label}</span>
      {[0, 1].map((row) => (
        <div className="flex min-h-20 items-center gap-4 px-3 py-4" key={row}>
          <div className="min-w-0 flex-1 space-y-2">
            <span className="product-skeleton block h-3.5 w-36 max-w-[55%]" />
            <span className="product-skeleton block h-3 w-56 max-w-[80%]" />
          </div>
          <span className="product-skeleton block h-9 w-20" />
        </div>
      ))}
    </div>
  );
}


function useManagementClose(dirty: boolean, close: () => void) {
  const [discarding, setDiscarding] = useState(false);
  return {
    requestClose: () => { if (dirty) setDiscarding(true); else close(); },
    confirmation: <ConfirmDialog open={discarding} title={translate('devices.discardChangesTitle')} description={translate('devices.discardChangesDescription')} confirmLabel={translate('devices.discardChanges')} onCancel={() => setDiscarding(false)} onConfirm={() => { setDiscarding(false); close(); }} />,
  };
}

function RelayDialog({ busy, children, description, onClose, title }: {
  busy: boolean;
  children: React.ReactNode;
  description: string;
  onClose: () => void;
  title: string;
}) {
  // The shared dialog registers nested modal layers, so desktop management
  // windows keep pointer input and focus inside the topmost share/isolation form.
  return <FormDialog className="device-management-dialog" busy={busy} title={title} description={description} onClose={onClose}>{children}</FormDialog>;
}

export function SharePermissionsDialog({
  busy,
  error,
  onClose: close,
  onSave,
  share,
}: {
  busy: boolean;
  error: string | null;
  onClose: () => void;
  onSave: (input: {
    label: string | null;
    threadAccess: RelayThreadAccessDto;
    workspaceAccess: RelayWorkspaceAccessDto;
    expiresAt: string | null;
  }) => void;
  share: RelaySessionShareDto;
}) {
  const { locale: i18nLocale } = useI18n();
  const [label, setLabel] = useState(share.label ?? '');
  const [threadAccess, setThreadAccess] = useState<RelayThreadAccessDto>(
    share.threadAccess,
  );
  const [workspaceAccess, setWorkspaceAccess] =
    useState<RelayWorkspaceAccessDto>(share.workspaceAccess);
  const [expiresAt, setExpiresAt] = useState(
    toDatetimeLocalValue(share.expiresAt),
  );
  const workspaceAccessLocked = !share.workspaceId;

  const { requestClose: onClose, confirmation } = useManagementClose(label !== (share.label ?? '') || threadAccess !== share.threadAccess || workspaceAccess !== share.workspaceAccess || expiresAt !== toDatetimeLocalValue(share.expiresAt), close);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSave({
      label: label.trim() || null,
      threadAccess,
      workspaceAccess: workspaceAccessLocked ? 'none' : workspaceAccess,
      expiresAt: fromDatetimeLocalValue(expiresAt),
    });
  }

  return (
    <RelayDialog
      busy={busy}
      description={translate("devices.canAccess", { value1: share.targetUsername, value2: relayShareTitleText(share) })}
      onClose={onClose}
      title={translate("devices.sharedThreadPermissions")}
    >
      <form
        className="mt-5"
        onSubmit={submit}
      >
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
        <div className="space-y-4">
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.label")}<input
              className="relay-input mt-2 min-h-11 w-full"
              data-dialog-initial-focus
              onChange={(event) => setLabel(event.target.value)}
              placeholder={translate("devices.optionalSharedThreadLabel")}
              value={label}
            />
          </label>
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.threadAccess")}<Select
              className="relay-input mt-2 min-h-11 w-full"
              onValueChange={(selectedValue) =>
                setThreadAccess(selectedValue as RelayThreadAccessDto)
              }
              value={threadAccess}
            >
              <option value="read">{translate("devices.viewOnly")}</option>
              <option value="control">{translate("devices.collaborator")}</option>
            </Select>
          </label>
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.workspaceAccess")}<Select
              className="relay-input mt-2 min-h-11 w-full"
              disabled={workspaceAccessLocked}
              onValueChange={(selectedValue) =>
                setWorkspaceAccess(
                  selectedValue as RelayWorkspaceAccessDto,
                )
              }
              value={workspaceAccessLocked ? 'none' : workspaceAccess}
            >
              <option value="none">{translate("devices.noWorkspace")}</option>
              <option value="read">{translate("devices.workspaceRead")}</option>
              <option value="write">{translate("devices.workspaceWrite")}</option>
            </Select>
          </label>
          {workspaceAccessLocked ? (
            <p className="rounded-md border border-[var(--theme-border)] bg-[var(--theme-surface)] px-3 py-2 text-xs text-[var(--theme-fg-muted)]">
              {translate("devices.thisShareWasCreatedWithoutAWorkspace")}</p>
          ) : null}
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.expiration")}<input
              aria-label={translate("devices.expiration")}
              className="relay-input mt-2 min-h-11 w-full"
              onChange={(event) => setExpiresAt(event.target.value)}
              type="datetime-local"
              value={expiresAt}
            />
            <span className="mt-1 block text-xs text-[var(--theme-fg-muted)]">
              {translate("devices.leaveEmptyForNoExpiration")}</span>
          </label>
        </div>
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <button
            className="relay-button-secondary min-h-11 w-full sm:w-auto"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            {translate("devices.cancel")}</button>
          <div className="flex items-center gap-2">
            <button
              className="relay-button-primary min-h-11 w-full min-w-0 sm:w-auto"
              disabled={busy}
              type="submit"
            >
              {busy ? translate("devices.saving") : translate("devices.savePermissions")}
            </button>
            {error ? <ErrorMark message={error} /> : null}
          </div>
        </div>
        </fieldset>
      </form>
      {confirmation}
    </RelayDialog>
  );
}

export function ShareDeviceDialog({
  busy,
  device,
  error,
  onClose: close,
  onShare,
}: {
  busy: boolean;
  device: RelayDeviceDto;
  error: string | null;
  onClose: () => void;
  onShare: (input: {
    targetIdentifier: string;
    label: string | null;
    threadAccess: RelayThreadAccessDto;
    workspaceAccess: RelayWorkspaceAccessDto;
    canCreateThreads: boolean;
  }) => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [targetIdentifier, setTargetIdentifier] = useState('');
  const [label, setLabel] = useState('');
  const [threadAccess, setThreadAccess] =
    useState<RelayThreadAccessDto>('read');
  const [workspaceAccess, setWorkspaceAccess] =
    useState<RelayWorkspaceAccessDto>('read');
  const [canCreateThreads, setCanCreateThreads] = useState(false);

  const { requestClose: onClose, confirmation } = useManagementClose(Boolean(targetIdentifier || label || threadAccess !== 'read' || workspaceAccess !== 'read' || canCreateThreads), close);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onShare({
      targetIdentifier: targetIdentifier.trim(),
      label: label.trim() || null,
      threadAccess,
      workspaceAccess,
      canCreateThreads,
    });
  }

  return (
    <RelayDialog
      busy={busy}
      description={translate("devices.giveAnotherRelayAccountAccessToThis")}
      onClose={onClose}
      title={translate("devices.share_928c7a", { value1: device.name })}
    >
      <form
        className="mt-5"
        onSubmit={submit}
      >
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
        <div className="space-y-4">
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.relayAccount")}<input
              className="relay-input mt-2 min-h-11 w-full"
              data-dialog-initial-focus
              onChange={(event) => setTargetIdentifier(event.target.value)}
              placeholder={translate("devices.usernameOrEmail")}
              value={targetIdentifier}
            />
          </label>
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.label")}<input
              className="relay-input mt-2 min-h-11 w-full"
              onChange={(event) => setLabel(event.target.value)}
              placeholder={translate("devices.optionalNoteShownInSharedDevicesBy")}
              value={label}
            />
          </label>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.threadAccess")}<Select
                className="relay-input mt-2 min-h-11 w-full"
                onValueChange={(selectedValue) =>
                  setThreadAccess(selectedValue as RelayThreadAccessDto)
                }
                value={threadAccess}
              >
                <option value="read">{translate("devices.viewOnly")}</option>
                <option value="control">{translate("devices.collaborator")}</option>
              </Select>
            </label>
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.workspaceAccess")}<Select
                className="relay-input mt-2 min-h-11 w-full"
                onValueChange={(selectedValue) =>
                  setWorkspaceAccess(
                    selectedValue as RelayWorkspaceAccessDto,
                  )
                }
                value={workspaceAccess}
              >
                <option value="none">{translate("devices.noWorkspace")}</option>
                <option value="read">{translate("devices.workspaceRead")}</option>
                <option value="write">{translate("devices.workspaceWrite")}</option>
              </Select>
            </label>
          </div>
          <label className="flex min-h-11 items-center justify-between gap-3 text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.canCreateNewThreads")}<input
              checked={canCreateThreads}
              className="h-4 w-4 accent-[var(--theme-accent)]"
              onChange={(event) => setCanCreateThreads(event.target.checked)}
              type="checkbox"
            />
          </label>
        </div>
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <button
            className="relay-button-secondary min-h-11 w-full sm:w-auto"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            {translate("devices.cancel")}</button>
          <div className="flex items-center gap-2">
            <button
              className="relay-button-primary inline-flex min-h-11 w-full min-w-0 items-center justify-center gap-2 sm:w-auto"
              disabled={busy || !targetIdentifier.trim()}
              type="submit"
            >
              <Share2 className="h-4 w-4" />
              {busy ? translate("devices.sharing") : translate("devices.shareDevice")}
            </button>
            {error ? <ErrorMark message={error} /> : null}
          </div>
        </div>
        </fieldset>
      </form>
      {confirmation}
    </RelayDialog>
  );
}

export function GrantPermissionsDialog({
  busy,
  error,
  grant,
  onClose: close,
  onSave,
}: {
  busy: boolean;
  error: string | null;
  grant: RelayAccessGrantDto;
  onClose: () => void;
  onSave: (input: {
    label: string | null;
    threadAccess: RelayThreadAccessDto;
    workspaceAccess: RelayWorkspaceAccessDto;
    canCreateThreads: boolean;
    expiresAt: string | null;
  }) => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [label, setLabel] = useState(grant.label ?? '');
  const [threadAccess, setThreadAccess] = useState<RelayThreadAccessDto>(
    grant.threadAccess,
  );
  const [workspaceAccess, setWorkspaceAccess] =
    useState<RelayWorkspaceAccessDto>(grant.workspaceAccess);
  const [canCreateThreads, setCanCreateThreads] = useState(
    grant.canCreateThreads,
  );
  const [expiresAt, setExpiresAt] = useState(
    toDatetimeLocalValue(grant.expiresAt),
  );
  const canCreateThreadsAvailable = grant.scope !== 'thread';
  const workspaceAccessLocked = grant.scope === 'thread' && !grant.workspaceId;

  const { requestClose: onClose, confirmation } = useManagementClose(label !== (grant.label ?? '') || threadAccess !== grant.threadAccess || workspaceAccess !== grant.workspaceAccess || canCreateThreads !== grant.canCreateThreads || expiresAt !== toDatetimeLocalValue(grant.expiresAt), close);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onSave({
      label: label.trim() || null,
      threadAccess,
      workspaceAccess: workspaceAccessLocked ? 'none' : workspaceAccess,
      canCreateThreads: canCreateThreadsAvailable ? canCreateThreads : false,
      expiresAt: fromDatetimeLocalValue(expiresAt),
    });
  }

  return (
    <RelayDialog
      busy={busy}
      description={translate("devices.canAccess", { value1: grant.targetUsername, value2: grantTitleText(grant) })}
      onClose={onClose}
      title={translate("devices.sharedAccessPermissions")}
    >
      <form
        className="mt-5"
        onSubmit={submit}
      >
        <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
        <div className="space-y-4">
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.label")}<input
              className="relay-input mt-2 min-h-11 w-full"
              data-dialog-initial-focus
              onChange={(event) => setLabel(event.target.value)}
              placeholder={translate("devices.optionalSharedAccessLabel")}
              value={label}
            />
          </label>
          <div className="grid gap-3 sm:grid-cols-2">
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.threadAccess")}<Select
                className="relay-input mt-2 min-h-11 w-full"
                onValueChange={(selectedValue) =>
                  setThreadAccess(selectedValue as RelayThreadAccessDto)
                }
                value={threadAccess}
              >
                <option value="read">{translate("devices.viewOnly")}</option>
                <option value="control">{translate("devices.collaborator")}</option>
              </Select>
            </label>
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.workspaceAccess")}<Select
                className="relay-input mt-2 min-h-11 w-full"
                disabled={workspaceAccessLocked}
                onValueChange={(selectedValue) =>
                  setWorkspaceAccess(
                    selectedValue as RelayWorkspaceAccessDto,
                  )
                }
                value={workspaceAccessLocked ? 'none' : workspaceAccess}
              >
                <option value="none">{translate("devices.noWorkspace")}</option>
                <option value="read">{translate("devices.workspaceRead")}</option>
                <option value="write">{translate("devices.workspaceWrite")}</option>
              </Select>
            </label>
          </div>
          {canCreateThreadsAvailable ? (
            <label className="flex min-h-11 items-center justify-between gap-3 text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.canCreateNewThreads")}<input
                checked={canCreateThreads}
                className="h-4 w-4 accent-[var(--theme-accent)]"
                onChange={(event) => setCanCreateThreads(event.target.checked)}
                type="checkbox"
              />
            </label>
          ) : null}
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.expiration")}<input
              aria-label={translate("devices.expiration")}
              className="relay-input mt-2 min-h-11 w-full"
              onChange={(event) => setExpiresAt(event.target.value)}
              type="datetime-local"
              value={expiresAt}
            />
            <span className="mt-1 block text-xs text-[var(--theme-fg-muted)]">
              {translate("devices.leaveEmptyForNoExpiration")}</span>
          </label>
        </div>
        <div className="mt-5 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
          <button
            className="relay-button-secondary min-h-11 w-full sm:w-auto"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            {translate("devices.cancel")}</button>
          <div className="flex items-center gap-2">
            <button
              className="relay-button-primary min-h-11 w-full min-w-0 sm:w-auto"
              disabled={busy}
              type="submit"
            >
              {busy ? translate("devices.saving") : translate("devices.savePermissions")}
            </button>
            {error ? <ErrorMark message={error} /> : null}
          </div>
        </div>
        </fieldset>
      </form>
      {confirmation}
    </RelayDialog>
  );
}

/** Whether the page holds a protected, encrypted connection to the device, read as it changes. */
function useTransportStatus(deviceId: string) {
  const [transportStatus, setTransportStatus] = useState(() => getTransportStatus(deviceId));
  useEffect(() => {
    setTransportStatus(getTransportStatus(deviceId));
    const update = (event: Event) => {
      const status = (event as CustomEvent<NonNullable<typeof transportStatus>>).detail;
      if (status.deviceId === deviceId) setTransportStatus(status);
    };
    window.addEventListener('remote-codex-transport', update);
    return () => window.removeEventListener('remote-codex-transport', update);
  }, [deviceId]);
  return transportStatus;
}

/**
 * A device's state under its name on its own page: online since or last seen,
 * the encrypted connection, its protection, and the isolation and shared-control
 * marks (what the phone's device row used to show, issue #115).
 */
export function DeviceStatusLine({ device, portal, isolated }: {
  device: RelayDeviceDto;
  portal: RelayPortalSummaryDto | null;
  /** Read by the page from the device while it is online. */
  isolated: boolean;
}) {
  useI18n();
  const transportStatus = useTransportStatus(device.id);
  const protection = useDeviceProtection(device.id, transportStatus);
  const activityText = device.connected
    ? device.connectedAt
      ? translate('devices.onlineSince', { value1: formatRelayTimestamp(device.connectedAt) })
      : translate("devices.onlineConnectedTimeUnavailable")
    : device.lastHeartbeatAt
      ? translate('devices.lastSeen', { value1: formatRelayTimestamp(device.lastHeartbeatAt) })
      : translate("devices.noHeartbeatRecorded");
  return (
    <div className="device-meta device-status-line flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-[var(--theme-fg-muted)]">
      {/* The one online mark of a device, as on its card and in the device switcher (§6); the words follow it. */}
      <DeviceStateMark showLabel={false} state={device.connected ? 'online' : 'offline'} />
      <span>{activityText}</span>
      <DeviceEncryptionStatus deviceId={device.id} online={device.connected} />
      <DeviceProtectionBadge device={device} protection={protection} />
      {isolated ? <IsolationMark kind="device" /> : null}
      {device.sharedControl ? <SharedControlMark shares={controlShares(portal, device.id)} /> : null}
    </div>
  );
}

/**
 * The device's own part of its Settings tab: its name, its connection to the
 * relay (the setup commands and the token) and its isolation. Copying a command
 * and replacing the token are settings, not menu items of the device.
 */
function DeviceGeneralSettings({ device, controlShares, renameSupported, onRename, copied, copyError, onCopySetup, onRotate, children }: {
  device: RelayDeviceDto;
  controlShares: ControlShare[];
  renameSupported: boolean;
  onRename: () => void;
  copied: boolean;
  copyError: string | null;
  onCopySetup: (platform: SupervisorPlatform) => void;
  onRotate: () => void;
  /** The new token's commands after a replacement, and the window that confirms one. */
  children?: React.ReactNode;
}) {
  useI18n();
  // Which of the two commands was asked for, so the confirmation shows on its button.
  const [platform, setPlatform] = useState<SupervisorPlatform | null>(null);
  const copy = (next: SupervisorPlatform) => { setPlatform(next); onCopySetup(next); };
  const copyButton = (next: SupervisorPlatform, label: string) => (
    <button className="relay-button-secondary inline-flex min-h-11 items-center gap-2 px-3 text-sm sm:min-h-10" onClick={() => copy(next)} type="button">
      {copied && platform === next ? <Check aria-hidden="true" className="h-4 w-4 text-[var(--status-success-fg)]" /> : <Copy aria-hidden="true" className="h-4 w-4" />}
      {label}
    </button>
  );
  return (
    <div className="device-settings-general divide-y divide-[var(--theme-border)]">
      <section aria-labelledby="device-settings-name" className="py-5 first:pt-0">
        <h3 className="text-sm font-semibold text-[var(--theme-fg)]" id="device-settings-name">{translate('devices.settingsName')}</h3>
        <div className="mt-2 flex min-w-0 flex-wrap items-center gap-2">
          <span className="min-w-0 truncate text-sm text-[var(--theme-fg)]" title={device.name}>{device.name}</span>
          <button className="relay-button-secondary inline-flex min-h-11 items-center gap-2 px-3 text-sm sm:min-h-10" disabled={!renameSupported} onClick={onRename} type="button">
            <Pencil aria-hidden="true" className="h-4 w-4" />
            {translate('devices.manageRename')}
          </button>
          {renameSupported ? null : <ErrorMark kind="unavailable" message={translate('desktopDevices.renameUnsupported')} />}
        </div>
      </section>
      <section aria-labelledby="device-settings-connection" className="py-5">
        <h3 className="text-sm font-semibold text-[var(--theme-fg)]" id="device-settings-connection">{translate('devices.settingsConnection')}</h3>
        <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">{translate('devices.settingsConnectionHint')}</p>
        {device.tokenPreview ? (
          <p className="mt-2 flex min-w-0 items-center gap-2 text-xs text-[var(--theme-fg-muted)]">
            <span className="shrink-0">{translate('devices.settingsToken')}</span>
            <code className="min-w-0 truncate font-mono text-[var(--theme-fg)]">{device.tokenPreview}</code>
          </p>
        ) : null}
        <div className="mt-3 flex flex-wrap items-center gap-2">
          {copyButton('unix', translate("devices.copySetupForMacOSLinux"))}
          {copyButton('windows', translate("devices.copySetupForWindows"))}
          {copyError ? <ErrorMark message={copyError} /> : null}
          <span className="basis-full text-xs text-[var(--status-success-fg)]" role="status">{copied ? translate("devices.setupCommandCopied") : ''}</span>
        </div>
        {children}
        <div className="mt-3 flex items-center gap-2">
          {/* Replacing the token cuts the device off until it is set up again: it looks dangerous before it is pressed (§11). */}
          <button className="device-danger-button inline-flex min-h-11 items-center gap-2 rounded-md border px-3 text-sm font-medium sm:min-h-10" onClick={onRotate} type="button">
            <KeyRound aria-hidden="true" className="h-4 w-4" />
            {translate("devices.replaceDeviceToken_268410")}
          </button>
        </div>
      </section>
      <section aria-labelledby="device-settings-isolation" className="py-5">
        <h3 className="text-sm font-semibold text-[var(--theme-fg)]" id="device-settings-isolation">{translate('devices.agentIsolation')}</h3>
        <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">{translate('devices.deviceIsolationDescription')}</p>
        <DeviceIsolationControl device={device} controlShares={controlShares} />
      </section>
    </div>
  );
}

/** The relay refuses this device's requests to the owner's other devices; tap
    shows which of my shares cause it. */
function SharedControlMark({ shares }: { shares: ControlShare[] }) {
  useI18n();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const detail = translate('devices.sharedControlDetail');
  return (
    <>
      <button
        aria-expanded={open}
        aria-haspopup="dialog"
        aria-label={translate('devices.sharedControl')}
        className="device-status-chip"
        data-tone="mark"
        onClick={() => setOpen(!open)}
        ref={trigger}
        title={detail}
        type="button"
      >
        <Link2Off aria-hidden="true" className="h-3.5 w-3.5" />
      </button>
      <AnchoredPopover anchor={trigger} label={translate('devices.sharedControl')} onClose={() => setOpen(false)} open={open} width={320}>
        <SharedControlReasons shares={shares} />
      </AnchoredPopover>
    </>
  );
}

/** Whether agents on other devices may reach this one; the device decides and enforces it. */
function DeviceIsolationControl({ device, controlShares, onChange, onBusy }: {
  device: RelayDeviceDto;
  controlShares: ControlShare[];
  onChange?: ((isolated: boolean) => void) | undefined;
  onBusy?: ((busy: boolean) => void) | undefined;
}) {
  useI18n();
  const [isolated, setIsolated] = useState<boolean | null>(null);
  const [busy, setBusyState] = useState(false);
  const setBusy = (value: boolean) => { setBusyState(value); onBusy?.(value); };
  // Why the state could not be read, or why the toggle failed: marks after the toggle.
  const [error, setError] = useState<string | null>(null);
  const [toggleError, setToggleError] = useState<string | null>(null);
  const [unsupported, setUnsupported] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const changed = useRef(onChange);
  changed.current = onChange;
  useEffect(() => {
    if (!device.connected) return;
    let active = true;
    setError(null);
    void fetchAgentIsolation(device.id)
      .then((value) => {
        if (!active) return;
        setIsolated(value.device);
        changed.current?.(value.device);
      })
      .catch((caught) => {
        if (!active) return;
        if (caught instanceof ApiError && caught.statusCode === 404) setUnsupported(true);
        else setError(errorMessage(caught, translate('devices.deviceIsolationOffline')));
      });
    return () => { active = false; };
  }, [device.id, device.connected, attempt]);

  async function toggle(next: boolean) {
    setBusy(true);
    setToggleError(null);
    try {
      const value = await setDeviceIsolation(next, device.id);
      setIsolated(value.device);
      changed.current?.(value.device);
    } catch (caught) {
      setToggleError(errorMessage(caught, translate('devices.deviceIsolationOffline')));
    } finally {
      setBusy(false);
    }
  }
  // Offline or too old a device: the toggle stays, disabled, and its mark says why.
  const reason = !device.connected ? translate('devices.deviceIsolationOffline')
    : unsupported ? translate('devices.deviceIsolationUpdate') : null;

  return (
    <div className="mt-4 text-sm">
      {!reason && isolated === null && !error ? (
        <p className="text-[var(--theme-fg-muted)]">{translate('devices.deviceIsolationLoading')}</p>
      ) : (
        <div className="flex items-center gap-2">
          <label className={`flex min-h-11 min-w-0 flex-1 items-center gap-3 rounded-md border border-[var(--theme-border)] px-3 py-2 ${isolated === null ? 'opacity-60' : 'cursor-pointer'}`}>
            <input
              checked={isolated ?? false}
              className="h-4 w-4"
              disabled={busy || isolated === null}
              onChange={(event) => void toggle(event.target.checked)}
              type="checkbox"
            />
            <span className="font-medium text-[var(--theme-fg)]">{translate('devices.isolateDevice')}</span>
          </label>
          {reason ? <ErrorMark kind="unavailable" message={reason} />
            : error && isolated === null ? <ErrorMark message={error} onRetry={() => setAttempt((value) => value + 1)} />
            : toggleError ? <ErrorMark message={toggleError} /> : null}
        </div>
      )}
      {device.sharedControl ? (
        <div className="mt-3 flex items-start gap-2">
          <Link2Off aria-hidden="true" className="mt-1 h-3.5 w-3.5 shrink-0 text-[var(--theme-fg-muted)]" />
          <SharedControlReasons shares={controlShares} />
        </div>
      ) : null}
    </div>
  );
}

/** The same isolation control in a window, opened from the device's ⋯ menu. */
export function DeviceIsolationDialog({
  device,
  controlShares,
  onChange,
  onClose,
}: {
  device: RelayDeviceDto;
  controlShares: ControlShare[];
  onChange?: ((isolated: boolean) => void) | undefined;
  onClose: () => void;
}) {
  useI18n();
  const [busy, setBusy] = useState(false);
  return (
    <RelayDialog
      busy={busy}
      description={translate('devices.deviceIsolationDescription')}
      onClose={onClose}
      title={`${translate('devices.agentIsolation')} · ${device.name}`}
    >
      <DeviceIsolationControl device={device} controlShares={controlShares} onChange={onChange} onBusy={setBusy} />
    </RelayDialog>
  );
}

function DeviceTokenPanel({ result, binding, onOpen }: {
  result: RelayCreateDeviceResultDto;
  binding: DeviceBindingFlags;
  onOpen?: (() => void) | undefined;
}) {
  const { locale: i18nLocale } = useI18n();
  const [platform, setPlatform] = useState<SupervisorPlatform>('unix');
  const command = relaySupervisorCommand(result.token, platform, binding);
  return (
    <section
      aria-live="polite"
      className="mt-4 rounded-lg border border-[var(--theme-accent-border)] bg-[var(--theme-accent-soft)] p-4"
    >
      <h2 className="text-base font-semibold text-[var(--theme-fg)]">
        {translate("devices.tokenCreatedFor")} {result.device.name}
      </h2>
      <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
        {translate("devices.youCanCopyThisSetupAgainFrom")}</p>
      <p className="mt-2 text-xs text-[var(--theme-fg-muted)]">{translate("devices.theCommandInstallsOrUpdatesTheLatest")}</p>
      <div className="mt-3">
        <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
          <p className="text-xs font-medium uppercase tracking-[0.14em] text-[var(--theme-fg-muted)]">
            {translate("devices.supervisorCommand")}</p>
          <div
            aria-label={translate("devices.supervisorCommandPlatform")}
            className="inline-flex rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-0.5"
            role="group"
          >
            <PlatformButton
              active={platform === 'unix'}
              onClick={() => setPlatform('unix')}
            >
              macOS &amp; Linux
            </PlatformButton>
            <PlatformButton
              active={platform === 'windows'}
              onClick={() => setPlatform('windows')}
            >
              Windows
            </PlatformButton>
          </div>
        </div>
        <CodeBlock
          copyLabel={`Copy ${platform === 'windows' ? 'Windows PowerShell' : 'macOS and Linux'} supervisor command`}
          label={platform === 'windows' ? 'PowerShell' : translate("devices.shell")}
          nested
          value={command}
        />
      </div>
      <DeviceSetupProgress deviceId={result.device.id} {...(onOpen ? { onOpen } : {})} />
    </section>
  );
}

function PlatformButton({
  active,
  children,
  onClick,
}: {
  active: boolean;
  children: React.ReactNode;
  onClick: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <button
      aria-pressed={active}
      className={`min-h-11 rounded-md px-2.5 text-xs font-medium transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] sm:min-h-9 ${
        active
          ? 'bg-[var(--theme-surface-strong)] text-[var(--theme-fg)] shadow-sm'
          : 'text-[var(--theme-fg-muted)] hover:bg-[var(--theme-hover)] hover:text-[var(--theme-fg)]'
      }`}
      onClick={onClick}
      type="button"
    >
      {children}
    </button>
  );
}

function CodeBlock({
  copyLabel = translate("devices.copy"),
  label,
  nested = false,
  value,
}: {
  copyLabel?: string;
  label: string;
  nested?: boolean;
  value: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const [copyState, setCopyState] = useState<
    'idle' | 'copied' | 'error'
  >('idle');

  useEffect(() => {
    if (copyState !== 'copied') {
      return;
    }
    const timeoutId = window.setTimeout(() => setCopyState('idle'), 1800);
    return () => window.clearTimeout(timeoutId);
  }, [copyState]);

  async function copy() {
    const clipboard = navigator.clipboard;
    if (!clipboard || typeof clipboard.writeText !== 'function') {
      setCopyState('error');
      return;
    }
    try {
      await clipboard.writeText(value);
      setCopyState('copied');
    } catch {
      setCopyState('error');
    }
  }

  return (
    <div className={nested ? '' : 'mt-3'}>
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="text-xs font-medium uppercase tracking-[0.14em] text-[var(--theme-fg-muted)]">
          {label}
        </p>
        <span className="flex items-center gap-1">
          <button
            aria-label={copyLabel}
            className="relay-button-secondary inline-flex min-h-11 items-center gap-1 px-2 text-xs sm:min-h-9"
            onClick={() => void copy()}
            type="button"
          >
            <Copy className="h-3.5 w-3.5" />
            {copyState === 'copied' ? translate("devices.copied") : translate("devices.copy")}
          </button>
          {copyState === 'error' ? <ErrorMark message={translate("devices.clipboardAccessFailedSelectTheTextAnd")} /> : null}
        </span>
      </div>
      <code className="block break-all rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] px-3 py-2 font-mono text-xs text-[var(--theme-fg)]">
        {value}
      </code>
      {copyState === 'copied' ? (
        <p
          className="mt-1 text-xs text-[var(--status-success-fg)]"
          role="status"
        >
          {translate("devices.copiedToClipboard")}</p>
      ) : null}
    </div>
  );
}

function formatRelayTimestamp(value: string | null | undefined) {
  return value ? new Date(value).toLocaleString(getLocale()) : translate("devices.unavailable");
}

function toDatetimeLocalValue(value: string | null | undefined) {
  if (!value) {
    return '';
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const pad = (part: number) => String(part).padStart(2, '0');
  return [
    date.getFullYear(),
    '-',
    pad(date.getMonth() + 1),
    '-',
    pad(date.getDate()),
    'T',
    pad(date.getHours()),
    ':',
    pad(date.getMinutes()),
  ].join('');
}

function fromDatetimeLocalValue(value: string) {
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  const date = new Date(trimmed);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export function relayShareTitleText(share: RelaySessionShareDto) {
  return stableShareThreadTitle(share) ?? (share.label?.trim() || translate("devices.sharedThread"));
}

export function relayShareWorkspaceLabel(share: RelaySessionShareDto) {
  return share.workspaceAccess === 'none'
    ? translate("devices.noWorkspaceAccess")
    : share.workspaceLabel?.trim() || translate("devices.workspaceUnavailable");
}

function stableShareThreadTitle(share: RelaySessionShareDto) {
  const threadTitle = share.threadTitle?.trim();
  if (!threadTitle) {
    return null;
  }
  const label = share.label?.trim();
  return label && threadTitle === label ? null : threadTitle;
}

function grantTitleText(grant: RelayAccessGrantDto) {
  return grant.deviceName?.trim() || translate("devices.sharedDevice");
}

function deviceGrantScopeText(grant: RelayAccessGrantDto) {
  if (grant.workspaceScope !== 'selected') {
    return translate("devices.entireDevice");
  }
  if (grant.workspaceLabel?.trim()) {
    return translate("devices.selectedWorkspace", { value1: grant.workspaceLabel.trim() });
  }
  const count = grant.workspaceIds.length;
  return translate("devices.selectedWorkspace_99995e", { value1: count, value2: count === 1 ? '' : 's' });
}

function revokeDescription(
  target:
    | { kind: 'grant'; grant: RelayAccessGrantDto }
    | { kind: 'share'; share: RelaySessionShareDto },
) {
  if (target.kind === 'share') {
    return `Revoke ${target.share.targetUsername}'s access to thread ${relayShareTitleText(target.share)}? Their shared link will stop working immediately.`;
  }
  const grant = target.grant;
  const range =
    grant.scope === 'thread'
      ? `workspace ${grant.workspaceLabel?.trim() || 'unavailable'}, thread ${stableGrantThreadTitle(grant) ?? 'unavailable'}`
      : grant.scope === 'workspace'
        ? `workspace ${grant.workspaceLabel?.trim() || 'unavailable'}`
        : deviceGrantScopeText(grant).toLowerCase();
  return `Revoke ${grant.targetUsername}'s access to ${grantTitleText(grant)}? Scope: ${range}. Access will stop immediately.`;
}

function revokeTargetBusy(
  target:
    | { kind: 'grant'; grant: RelayAccessGrantDto }
    | { kind: 'share'; share: RelaySessionShareDto }
    | null,
  busy: string | null,
) {
  if (!target) {
    return false;
  }
  return (
    busy ===
    `${target.kind}:${target.kind === 'grant' ? target.grant.id : target.share.id}`
  );
}

function stableGrantThreadTitle(grant: RelayAccessGrantDto) {
  if (grant.scope !== 'thread') {
    return grant.threadTitle?.trim() || null;
  }
  const threadTitle = grant.threadTitle?.trim();
  if (!threadTitle) {
    return null;
  }
  const label = grant.label?.trim();
  return label && threadTitle === label ? null : threadTitle;
}

function grantScopeLabel(grant: RelayAccessGrantDto) {
  switch (grant.scope) {
    case 'device':
      return translate("devices.device");
    case 'workspace':
      return translate("devices.workspace");
    case 'thread':
    default:
      return translate("devices.thread");
  }
}

function workspaceAccessLabel(access: RelayWorkspaceAccessDto) {
  switch (access) {
    case 'write':
      return translate("devices.workspaceWrite");
    case 'read':
      return translate("devices.workspaceRead");
    case 'none':
    default:
      return translate("devices.noWorkspace");
  }
}
