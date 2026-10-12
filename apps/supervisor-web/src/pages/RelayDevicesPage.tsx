import { SignedShareDetails, OrphanDeviceGrants } from '../components/SignedShareDetails';
import { requireUnlock } from '../lib/accountVault';
import { removeDeviceFromAccount } from '../lib/deviceRemoval';
import { prepareDeviceEnrollment } from '../lib/deviceEnrollment';
import {
  relaySupervisorCommand,
  validateDeviceSetupToken,
  type SupervisorPlatform,
  type DeviceBindingFlags,
} from '../lib/deviceSetupCommands';
import { DeviceBindingPanel } from '../components/DeviceBindingPanel';
import { FormDialog } from '../components/FormDialog';
import { useDeviceProtection, protectionLabel } from '../lib/useDeviceProtection';
import { getTransportStatus } from '../lib/relayTransport';
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { SharedAccessCard } from '../components/SharedAccessCard';
import { SecurityVerification } from '../components/RelaySecurity';
import { securityRequest, type SecurityStatus } from '../lib/relaySecurity';
import { request } from '../lib/api';
import { DeviceEncryptionStatus } from '../components/DeviceEncryptionStatus';
import { TourLauncherButton } from '../components/onboarding/TourProvider';
import { ProductHeader } from '../components/ProductHeader';
import {
  Copy,
  Ellipsis,
  MonitorSmartphone,
  Plug,
  Plus,
  Share2,
  Trash2,
  X,
} from 'lucide-react';
import {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
} from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';

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
  fetchRelayPortal,
  revokeRelayGrant,
  revokeRelayShare,
  setSelectedRelayDeviceId,
  setSelectedRelayThreadId,
  updateRelayGrant,
  updateRelayShare,
} from '../lib/api';
import { threadHref, workspacesHref } from '../lib/relayRoutes';
import { ConfirmDialog } from '../components/ConfirmDialog';
import { useDevicePresence } from '../lib/useDevicePresence';

const RELAY_PORTAL_REFRESH_INTERVAL_MS = 3000;
type SharedView =
  | 'incoming-threads'
  | 'incoming-devices'
  | 'outgoing-devices'
  | 'outgoing-threads';

function errorMessage(caught: unknown, fallback: string) {
  return caught instanceof ApiError
    ? caught.payload.message
    : caught instanceof Error
      ? caught.message
      : fallback;
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

export function RelayDevicesPage() {
  const { locale: i18nLocale } = useI18n();
  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const [rawPortal, setPortal] = useState<RelayPortalSummaryDto | null>(null);
  const portal = useDevicePresence(rawPortal);
  const [rotatingDevice, setRotatingDevice] = useState<RelayDeviceDto | null>(null);
  const [rotationVerification,setRotationVerification] = useState<SecurityStatus | null>(null);
  async function rotateDevice(verified=false) {
    if (!rotatingDevice) return;
    try {
      if (!verified) {const status=await securityRequest<SecurityStatus>(''); if (!status.recentlyVerified) {setRotationVerification(status);return;}}
      const binding = await prepareDeviceEnrollment(rotatingDevice.id);
      setCreatedBinding(binding);
      const result=await request<RelayCreateDeviceResultDto>(`/relay/devices/${rotatingDevice.id}/token`,{method:'POST'});
      validateDeviceSetupToken(result.token);
      setCreatedDevice(result);setAddDeviceOpen(true);setRotatingDevice(null);setRotationVerification(null);await load({showLoading:false});
    } catch(e) {setError(errorMessage(e,translate("devices.tokenRotationFailed")));}
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
  const [error, setError] = useState<string | null>(null);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [activeSharedView, setActiveSharedView] =
    useState<SharedView>('incoming-threads');
  const [expandedShareId, setExpandedShareId] = useState<string | null>(null);
  const [expandedGrantId, setExpandedGrantId] = useState<string | null>(null);
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
  const [addDeviceOpen, setAddDeviceOpen] = useState(false);
  const hasLoadedPortalRef = useRef(false);
  const handledShareDeviceRequestRef = useRef<string | null>(null);
  const copiedResetTimeoutRef = useRef<number | null>(null);
  const addDeviceButtonRef = useRef<HTMLButtonElement>(null);

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
        setRefreshError(null);
        return true;
      } catch (caught) {
        const message = errorMessage(caught, translate("devices.unableToLoadDevices"));
        if (showLoading || !hasLoadedPortalRef.current) {
          setError(message);
        } else {
          setRefreshError(message);
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

  async function addDevice(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setBusy('create');
    setError(null);
    try {
      await requireUnlock('bind-device');
      const result = await createRelayDevice({ name: deviceName });
      validateDeviceSetupToken(result.token);
      setCreatedBinding(await prepareDeviceEnrollment(result.device.id));
      setCreatedDevice(result);
      setDeviceName('');
      setAddDeviceOpen(false);
      await load({ showLoading: false });
    } catch (caught) {
      setError(errorMessage(caught, translate("devices.unableToCreateDevice")));
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
      setDeletingDevice(null);
      window.requestAnimationFrame(() => {
        window.requestAnimationFrame(() => {
          if (document.activeElement === document.body) {
            document.getElementById('devices-heading')?.focus();
          }
        });
      });
    } catch (caught) {
      setError(errorMessage(caught, translate("devices.unableToDeleteDevice")));
    } finally {
      setBusy(null);
    }
  }

  function connectDevice(device: RelayDeviceDto) {
    setSelectedRelayDeviceId(device.id);
    setSelectedRelayThreadId(null);
    navigate(workspacesHref(device.id));
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
      setSharingDevice(null);
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
      });
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
      setExpandedGrantId((current) => (current === grant.id ? null : current));
      setRevokeTarget(null);
      await load({ showLoading: false });
    } catch (caught) {
      setError(errorMessage(caught, translate("devices.unableToRemoveSharedAccess")));
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
      });
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

  async function revokeSharedSession(share: RelaySessionShareDto) {
    setBusy(`share:${share.id}`);
    setError(null);
    try {
      await revokeRelayShare(share.id);
      setExpandedShareId((current) => (current === share.id ? null : current));
      setRevokeTarget(null);
      await load({ showLoading: false });
    } catch (caught) {
      setError(errorMessage(caught, translate("devices.unableToRemoveSharedThreadAccess")));
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
        const { token } = await request<{ token: string }>(`/relay/devices/${device.id}/setup-token`, { method: 'POST' });
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
    } catch {
      setCopiedDeviceId(null);
      setDeviceCopyError({
        deviceId: device.id,
        message:
          translate("devices.unableToCopyTheSetupCommandCheck"),
      });
    }
  }

  const sharedDevicesWithMe = portal?.sharedDevicesWithMe ?? [];
  const outgoingGrants = portal?.grantsByMe ?? [];
  const sharedViewTabs: Array<{
    count: number;
    id: SharedView;
    label: string;
  }> = [
    {
      id: 'incoming-threads',
      label: translate("devices.threadsWithMe"),
      count: portal?.sharedWithMe.length ?? 0,
    },
    {
      id: 'incoming-devices',
      label: translate("devices.devicesWithMe"),
      count: groupGrantsByDevice(sharedDevicesWithMe).length,
    },
    {
      id: 'outgoing-devices',
      label: translate("devices.devicesByMe"),
      count: groupGrantsByDevice(outgoingGrants).length,
    },
    {
      id: 'outgoing-threads',
      label: translate("devices.threadsByMe"),
      count: portal?.sharedByMe.length ?? 0,
    },
  ];

  function handleSharedTabKeyDown(
    event: ReactKeyboardEvent<HTMLButtonElement>,
    index: number,
  ) {
    let nextIndex: number | null = null;
    if (event.key === 'ArrowRight') {
      nextIndex = (index + 1) % sharedViewTabs.length;
    } else if (event.key === 'ArrowLeft') {
      nextIndex =
        (index - 1 + sharedViewTabs.length) % sharedViewTabs.length;
    } else if (event.key === 'Home') {
      nextIndex = 0;
    } else if (event.key === 'End') {
      nextIndex = sharedViewTabs.length - 1;
    }
    if (nextIndex === null) {
      return;
    }
    event.preventDefault();
    const nextView = sharedViewTabs[nextIndex]?.id;
    if (!nextView) {
      return;
    }
    setActiveSharedView(nextView);
    document.getElementById(`shared-tab-${nextView}`)?.focus();
  }

  return (
    <div>
      <div className="product-page space-y-6" data-page="devices">
        <ProductHeader title={translate("devices.devices")} backHref="/" backLabel={translate("devices.relayHome")} actions={<TourLauncherButton showLabel />} />

        <section className="product-page-header">
          <div>
            <h1 className="product-title">
              {translate("devices.devicesAndSharedSessions")}</h1>
          </div>
        </section>

        {error ? <Notice tone="danger">{error}</Notice> : null}
        {refreshError ? (
          <Notice tone="danger">
            {translate("devices.latestDeviceStatusCouldNotBeRefreshed")} {refreshError}
            {translate("devices.retryingAutomatically")}
          </Notice>
        ) : null}
        <section aria-labelledby="devices-heading">
          <div className="flex items-start justify-between gap-4">
            <div className="min-w-0">
              <h2
                className="text-lg font-semibold text-[var(--theme-fg)]"
                id="devices-heading"
                tabIndex={-1}
              >
                {translate("devices.devices")}</h2>
              <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
                {translate("devices.yourRelaySupervisorsAndTheirCurrentAvailability")}</p>
            </div>
            <button
              aria-controls="add-device-form"
              aria-expanded={addDeviceOpen}
              className="relay-button-secondary inline-flex min-h-11 shrink-0 items-center gap-2 px-3 sm:min-h-10"
              onClick={() => {
                setError(null);
                setAddDeviceOpen((current) => !current);
              }}
              ref={addDeviceButtonRef}
              type="button"
            >
              {addDeviceOpen ? (
                <X className="h-4 w-4" />
              ) : (
                <Plus className="h-4 w-4" />
              )}
              {addDeviceOpen ? translate("devices.close") : translate("devices.addDevice")}
            </button>
          </div>

          {addDeviceOpen ? (
            <AddDeviceForm
              busy={busy === 'create'}
              deviceName={deviceName}
              onChangeDeviceName={setDeviceName}
              onClose={() => {
                setAddDeviceOpen(false);
                window.requestAnimationFrame(() =>
                  addDeviceButtonRef.current?.focus(),
                );
              }}
              onSubmit={addDevice}
            />
          ) : null}

          {createdDevice && createdBinding ? <DeviceTokenPanel result={createdDevice} binding={createdBinding} /> : null}

          <div className="mt-4 rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)]">
            <div className="hidden grid-cols-[minmax(14rem,1fr)_minmax(14rem,0.8fr)_auto] gap-5 border-b border-[var(--theme-border)] px-3 py-2 text-xs font-medium text-[var(--theme-fg-muted)] md:grid">
              <span>{translate("devices.device")}</span>
              <span>{translate("devices.activity")}</span>
              <span className="text-right">{translate("devices.action")}</span>
            </div>
            {loading ? (
              <LoadingRows label={translate("devices.loadingDevices")} />
            ) : portal?.devices.length ? (
              <div className="divide-y divide-[var(--theme-border)]">
                {portal.devices.map((device) => (
                  <DeviceRow
                    busy={busy === device.id}
                    copiedSetup={copiedDeviceId === device.id}
                    copyError={
                      deviceCopyError?.deviceId === device.id
                        ? deviceCopyError.message
                        : null
                    }
                    device={device}
                    key={device.id}
                    onRotate={() => setRotatingDevice(device)}
                    onConnect={() => connectDevice(device)}
                    onCopySetup={(platform) =>
                      void copySupervisorSetup(device, platform)
                    }
                    onDelete={() => {
                      setError(null);
                      setDeletingDevice(device);
                    }}
                    onShare={() => {
                      setDialogError(null);
                      setSharingDevice(device);
                    }}
                  />
                ))}
              </div>
            ) : (
              <div className="product-empty">
                {translate("devices.noDevicesYetAddADeviceTo")}</div>
            )}
          </div>
        </section>

        <section aria-labelledby="shared-access-heading" className="pt-2">
          <div>
            <h2
              className="text-lg font-semibold text-[var(--theme-fg)]"
              id="shared-access-heading"
            >
              {translate("devices.sharedAccess")}</h2>
            <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
              {translate("devices.accessYouReceivedAndAccessYouGranted")}</p>
          </div>
          <div
            aria-label={translate("devices.sharedAccessViews")}
            className="product-segmented shared-access-tabs mt-4"
            role="tablist"
          >
            {sharedViewTabs.map((tab, index) => {
              const selected = tab.id === activeSharedView;
              return (
                <button
                  aria-controls={`shared-panel-${tab.id}`}
                  aria-selected={selected}
                  className="product-segment inline-flex min-h-11 shrink-0 items-center gap-2 transition focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--theme-accent-ring)]"
                  id={`shared-tab-${tab.id}`}
                  key={tab.id}
                  onClick={() => setActiveSharedView(tab.id)}
                  onKeyDown={(event) =>
                    handleSharedTabKeyDown(event, index)
                  }
                  role="tab"
                  style={{ minHeight: '2.75rem' }}
                  tabIndex={selected ? 0 : -1}
                  type="button"
                >
                  {tab.label}
                  <span className="rounded-full bg-[var(--theme-muted)] px-2 py-0.5 text-xs text-[var(--theme-fg-muted)]">
                    {tab.count}
                  </span>
                </button>
              );
            })}
          </div>

          <div
            aria-labelledby={`shared-tab-${activeSharedView}`}
            id={`shared-panel-${activeSharedView}`}
            role="tabpanel"
            tabIndex={0}
          >
            {portal && (activeSharedView === 'outgoing-threads' || activeSharedView === 'outgoing-devices') && <OrphanDeviceGrants portal={portal} kind={activeSharedView === 'outgoing-threads' ? 'share' : 'grant'} />}
            {activeSharedView === 'incoming-threads' ? (
            <ShareSection
              count={portal?.sharedWithMe.length ?? 0}
              emptyText="No sessions have been shared with this account yet."
              loading={loading}
              loadingText="Loading shared sessions..."
              shares={portal?.sharedWithMe ?? []}
              title={translate("devices.sharedWithMe")}
              subtitle="Sessions another relay user has shared with this account."
              renderShare={(share) => (
                <SharedSessionRow
                  key={share.id}
                  mode="incoming"
                  share={share}
                  onOpen={() => openSharedSession(share)}
                />
              )}
            />
            ) : null}
            {activeSharedView === 'incoming-devices' ? (
            <GrantSection
              emptyText="No devices have been shared with this account yet."
              grants={sharedDevicesWithMe}
              loading={loading}
              loadingText="Loading shared devices..."
              title={translate("devices.sharedDevices")}
              subtitle="Devices another relay user has shared with this account."
              renderDevice={(group) => (
                <GrantDeviceCard
                  key={group.deviceId}
                  grants={group.grants}
                  mode="incoming"
                  onOpen={openSharedGrant}
                />
              )}
            />
            ) : null}
            {activeSharedView === 'outgoing-devices' ? (
            <GrantSection
              emptyText="No devices have been shared by this account yet."
              grants={outgoingGrants}
              loading={loading}
              loadingText="Loading shared devices..."
              title={translate("devices.sharedDevicesByMe")}
              subtitle="Devices this relay account has shared with other users."
              renderDevice={(group) => (
                <GrantDeviceCard
                  busyGrantId={
                    busy?.startsWith('grant:') ? busy.slice(6) : null
                  }
                  expandedGrantId={expandedGrantId}
                  key={group.deviceId}
                  grants={group.grants}
                  mode="outgoing"
                  onOpen={openSharedGrant}
                  onEdit={(grant) => {
                    setDialogError(null);
                    setEditingGrant(grant);
                  }}
                  onRevoke={(grant) => {
                    setError(null);
                    setRevokeTarget({ kind: 'grant', grant });
                  }}
                  onToggleAccess={(grant) => {
                    setExpandedGrantId((current) =>
                      current === grant.id ? null : grant.id,
                    );
                  }}
                />
              )}
            />
            ) : null}
            {activeSharedView === 'outgoing-threads' ? (
            <ShareSection
              count={portal?.sharedByMe.length ?? 0}
              emptyText="No threads have been shared by this account yet."
              loading={loading}
              loadingText="Loading shared threads..."
              shares={portal?.sharedByMe ?? []}
              title={translate("devices.sharedThreadsByMe")}
              subtitle="Threads this relay account has shared with other users."
              renderShare={(share) => (
                <SharedSessionRow
                  busy={busy === `share:${share.id}`}
                  expanded={expandedShareId === share.id}
                  key={share.id}
                  mode="outgoing"
                  share={share}
                  onOpen={() => openSharedSession(share)}
                  onEdit={() => {
                    setDialogError(null);
                    setEditingShare(share);
                  }}
                  onRevoke={() => {
                    setError(null);
                    setRevokeTarget({ kind: 'share', share });
                  }}
                  onToggleAccess={() => {
                    setExpandedShareId((current) =>
                      current === share.id ? null : share.id,
                    );
                  }}
                />
              )}
            />
            ) : null}
          </div>
        </section>
      </div>
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
            setSharingDevice(null);
            setDialogError(null);
          }}
          onShare={(input) => void createDeviceGrant(sharingDevice, input)}
        />
      ) : null}
      <ConfirmDialog open={Boolean(rotatingDevice) && !rotationVerification} title={translate("devices.replaceDeviceToken")} description={translate("devices.theCurrentConnectionWillCloseUpdateThe")} confirmLabel={translate("devices.replaceToken")} onConfirm={() => void rotateDevice()} onCancel={() => setRotatingDevice(null)} />
      {rotationVerification && <SecurityVerification status={rotationVerification} onCancel={()=> {setRotationVerification(null);setRotatingDevice(null);}} onVerified={()=>rotateDevice(true)} />}
      <ConfirmDialog
        open={deletingDevice !== null}
        title={translate("devices.removeDevice")}
        description={
          deletingDevice
            ? translate("devices.removeDeviceHelp", { value1: deletingDevice.name })
            : ''
        }
        confirmLabel={translate("devices.removeDevice")}
        busyLabel={translate("devices.deleting")}
        busy={Boolean(deletingDevice && busy === deletingDevice.id)}
        error={deletingDevice ? error : null}
        onCancel={() => {
          if (!deletingDevice || busy !== deletingDevice.id) {
            setDeletingDevice(null);
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
        error={revokeTarget ? error : null}
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
    </div>
  );
}

function AddDeviceForm({
  busy,
  deviceName,
  onChangeDeviceName,
  onClose,
  onSubmit,
}: {
  busy: boolean;
  deviceName: string;
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
        <div className="flex shrink-0 gap-2">
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

function ShareSection({
  count,
  emptyText,
  loading,
  loadingText,
  renderShare,
  shares,
  subtitle,
  title,
}: {
  count: number;
  emptyText: string;
  loading: boolean;
  loadingText: string;
  renderShare: (share: RelaySessionShareDto) => React.ReactNode;
  shares: RelaySessionShareDto[];
  subtitle: string;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <section className="pt-5">
      <div className="mb-3 flex items-start justify-between gap-3 px-1">
        <div>
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">
            {title}
          </h2>
          <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
            {subtitle}
          </p>
        </div>
        <span className="rounded-full bg-[var(--theme-muted)] px-2 py-0.5 text-xs text-[var(--theme-fg-muted)]">
          {count}
        </span>
      </div>
      {loading ? (
        <div className="product-list">
          <LoadingRows label={loadingText} />
        </div>
      ) : shares.length ? (
        <div className="product-list divide-y divide-[var(--theme-border)]">
          {shares.map((share) => renderShare(share))}
        </div>
      ) : (
        <div className="product-empty product-list">
          {emptyText}
        </div>
      )}
    </section>
  );
}

interface GrantDeviceGroup {
  deviceId: string;
  grants: RelayAccessGrantDto[];
}

function groupGrantsByDevice(
  grants: RelayAccessGrantDto[],
): GrantDeviceGroup[] {
  const grouped = new Map<string, RelayAccessGrantDto[]>();
  for (const grant of grants) {
    const deviceGrants = grouped.get(grant.deviceId);
    if (deviceGrants) {
      deviceGrants.push(grant);
    } else {
      grouped.set(grant.deviceId, [grant]);
    }
  }
  const scopeOrder: Record<RelayAccessGrantDto['scope'], number> = {
    device: 0,
    workspace: 1,
    thread: 2,
  };
  return [...grouped].map(([deviceId, deviceGrants]) => ({
    deviceId,
    grants: [...deviceGrants].sort(
      (left, right) =>
        scopeOrder[left.scope] - scopeOrder[right.scope] ||
        (left.workspaceLabel ?? '').localeCompare(right.workspaceLabel ?? '') ||
        (left.threadTitle ?? '').localeCompare(right.threadTitle ?? ''),
    ),
  }));
}

function GrantSection({
  emptyText,
  grants,
  loading,
  loadingText,
  renderDevice,
  subtitle,
  title,
}: {
  emptyText: string;
  grants: RelayAccessGrantDto[];
  loading: boolean;
  loadingText: string;
  renderDevice: (group: GrantDeviceGroup) => React.ReactNode;
  subtitle: string;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const deviceGroups = groupGrantsByDevice(grants);
  return (
    <section className="pt-5">
      <div className="mb-3 flex items-start justify-between gap-3 px-1">
        <div>
          <h2 className="text-base font-semibold text-[var(--theme-fg)]">
            {title}
          </h2>
          <p className="mt-1 text-sm text-[var(--theme-fg-muted)]">
            {subtitle}
          </p>
        </div>
        <span className="rounded-full bg-[var(--theme-muted)] px-2 py-0.5 text-xs text-[var(--theme-fg-muted)]">
          {deviceGroups.length}
        </span>
      </div>
      {loading ? (
        <div className="product-list">
          <LoadingRows label={loadingText} />
        </div>
      ) : deviceGroups.length ? (
        <div className="product-list divide-y divide-[var(--theme-border)]">
          {deviceGroups.map((group) => renderDevice(group))}
        </div>
      ) : (
        <div className="product-empty product-list">
          {emptyText}
        </div>
      )}
    </section>
  );
}

function SharedSessionRow({
  busy = false,
  expanded = false,
  mode,
  onEdit,
  onRevoke,
  onToggleAccess,
  share,
  onOpen,
}: {
  busy?: boolean;
  expanded?: boolean;
  mode: 'incoming' | 'outgoing';
  onEdit?: () => void;
  onRevoke?: () => void;
  onToggleAccess?: () => void;
  share: RelaySessionShareDto;
  onOpen?: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  return <SharedAccessCard title={relayShareTitleText(share)}
    subtitle={relayShareWorkspaceLabel(share)}
    deviceName={share.deviceName} deviceConnected={share.deviceConnected}
    username={mode === 'incoming' ? share.ownerUsername : share.targetUsername} mode={mode}
    permissions={[share.threadAccess === 'read' ? translate("devices.viewOnly") : translate("devices.collaborator"), workspaceAccessLabel(share.workspaceAccess)]}
    events={share.accessEvents} lastAccessedAt={share.lastAccessedAt} busy={busy} expanded={expanded}
    onOpen={onOpen} onEdit={onEdit} onRevoke={onRevoke} onToggleAccess={onToggleAccess} details={mode === 'outgoing' ? <SignedShareDetails kind="share" row={share} /> : undefined} />;
}

function GrantDeviceCard({
  busyGrantId = null,
  expandedGrantId = null,
  grants,
  mode,
  onEdit,
  onOpen,
  onRevoke,
  onToggleAccess,
}: {
  busyGrantId?: string | null;
  expandedGrantId?: string | null;
  grants: RelayAccessGrantDto[];
  mode: 'incoming' | 'outgoing';
  onEdit?: (grant: RelayAccessGrantDto) => void;
  onOpen?: (grant: RelayAccessGrantDto) => void;
  onRevoke?: (grant: RelayAccessGrantDto) => void;
  onToggleAccess?: (grant: RelayAccessGrantDto) => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const firstGrant = grants[0];
  if (!firstGrant) {
    return null;
  }
  const deviceName = grantTitleText(firstGrant);

  return (
    <article className="px-3 py-4">
      <header className="flex items-center justify-between gap-3">
        <p className="min-w-0 truncate text-sm font-semibold text-[var(--theme-fg)]">
          {deviceName}
        </p>
        <span className="shrink-0 rounded-full bg-[var(--theme-muted)] px-2 py-0.5 text-[11px] text-[var(--theme-fg-muted)]">
          {grants.length} {grants.length === 1 ? translate("devices.share") : translate("devices.shares")}
        </span>
      </header>
      <div className="mt-3 divide-y divide-[var(--theme-border)] border-t border-[var(--theme-border)]">
        {grants.map((grant) => (
          <GrantScopeRow
            busy={busyGrantId === grant.id}
            expanded={expandedGrantId === grant.id}
            grant={grant}
            key={grant.id}
            mode={mode}
            {...(onEdit ? { onEdit: () => onEdit(grant) } : {})}
            {...(onOpen ? { onOpen: () => onOpen(grant) } : {})}
            {...(onRevoke ? { onRevoke: () => onRevoke(grant) } : {})}
            {...(onToggleAccess
              ? { onToggleAccess: () => onToggleAccess(grant) }
              : {})}
          />
        ))}
      </div>
    </article>
  );
}

function GrantScopeRow({
  busy,
  expanded,
  grant,
  mode,
  onEdit,
  onOpen,
  onRevoke,
  onToggleAccess,
}: {
  busy: boolean;
  expanded: boolean;
  grant: RelayAccessGrantDto;
  mode: 'incoming' | 'outgoing';
  onEdit?: () => void;
  onOpen?: () => void;
  onRevoke?: () => void;
  onToggleAccess?: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  return <SharedAccessCard deviceName={grant.deviceName} deviceConnected={grant.deviceConnected} title={grant.label?.trim() || (grant.scope === 'thread' ? stableGrantThreadTitle(grant) : grant.scope === 'workspace' ? grant.workspaceLabel : grant.deviceName) || grantScopeLabel(grant)}
    subtitle={`${grantScopeLabel(grant)} · ${grant.scope === 'device' ? deviceGrantScopeText(grant) : grant.workspaceLabel || grant.deviceName}`}
    username={mode === 'incoming' ? grant.ownerUsername : grant.targetUsername} mode={mode}
    permissions={[grant.threadAccess === 'read' ? translate("devices.viewOnly") : translate("devices.collaborator"), workspaceAccessLabel(grant.workspaceAccess), ...(grant.canCreateThreads ? [translate("devices.canCreateThreads")] : [])]}
    events={grant.accessEvents} lastAccessedAt={grant.lastAccessedAt} busy={busy} expanded={expanded}
    onOpen={onOpen} onEdit={onEdit} onRevoke={onRevoke} onToggleAccess={onToggleAccess} details={mode === 'outgoing' ? <SignedShareDetails kind="grant" row={grant} /> : undefined} />;
}

const DIALOG_FOCUSABLE_SELECTOR = [
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  'a[href]',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

function useAccessibleDialog({
  busy,
  onClose,
}: {
  busy: boolean;
  onClose: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const dialogRef = useRef<HTMLDivElement>(null);
  const busyRef = useRef(busy);
  const closeRef = useRef(onClose);
  busyRef.current = busy;
  closeRef.current = onClose;

  useEffect(() => {
    const dialog = dialogRef.current;
    const previouslyFocused =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const previousOverflow = document.body.style.overflow;
    document.body.style.overflow = 'hidden';

    if (dialog && !dialog.contains(document.activeElement)) {
      const initialTarget = dialog.querySelector<HTMLElement>(
        '[data-dialog-initial-focus]',
      );
      const firstTarget = dialog.querySelector<HTMLElement>(
        DIALOG_FOCUSABLE_SELECTOR,
      );
      (initialTarget ?? firstTarget ?? dialog).focus();
    }

    function handleKeyDown(event: KeyboardEvent) {
      if (!dialog) {
        return;
      }
      if (event.key === 'Escape' && !busyRef.current) {
        event.preventDefault();
        closeRef.current();
        return;
      }
      if (event.key !== 'Tab') {
        return;
      }

      const focusable = Array.from(
        dialog.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE_SELECTOR),
      );
      if (!focusable.length) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (
        event.shiftKey &&
        (document.activeElement === first ||
          !dialog.contains(document.activeElement))
      ) {
        event.preventDefault();
        last?.focus();
      } else if (
        !event.shiftKey &&
        (document.activeElement === last ||
          !dialog.contains(document.activeElement))
      ) {
        event.preventDefault();
        first?.focus();
      }
    }

    document.addEventListener('keydown', handleKeyDown);
    return () => {
      document.removeEventListener('keydown', handleKeyDown);
      document.body.style.overflow = previousOverflow;
      previouslyFocused?.focus();
    };
  }, []);

  return dialogRef;
}

function RelayDialog({
  busy,
  children,
  description,
  error,
  onClose,
  title,
}: {
  busy: boolean;
  children: React.ReactNode;
  description: string;
  error: string | null;
  onClose: () => void;
  title: string;
}) {
  const { locale: i18nLocale } = useI18n();
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useAccessibleDialog({ busy, onClose });

  return (
    <div
      className="fixed inset-0 z-[90] flex items-center justify-center bg-[color-mix(in_oklch,var(--app-bg)_82%,transparent)] p-3 sm:p-6"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) {
          onClose();
        }
      }}
    >
      <div
        aria-describedby={descriptionId}
        aria-labelledby={titleId}
        aria-modal="true"
        className="product-dialog max-h-[calc(100dvh-1.5rem)] w-full max-w-lg overflow-y-auto rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-4 shadow-2xl sm:max-h-[min(42rem,calc(100dvh-3rem))] sm:p-5"
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
      >
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0">
            <h2
              className="text-base font-semibold text-[var(--theme-fg)]"
              id={titleId}
            >
              {title}
            </h2>
            <p
              className="mt-1 text-sm text-[var(--theme-fg-muted)]"
              id={descriptionId}
            >
              {description}
            </p>
          </div>
          <button
            aria-label={translate("devices.close_069e97", { value1: title })}
            className="product-icon-button"
            disabled={busy}
            onClick={onClose}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        {error ? (
          <div className="mt-4">
            <Notice tone="danger">{error}</Notice>
          </div>
        ) : null}
        {children}
      </div>
    </div>
  );
}

function SharePermissionsDialog({
  busy,
  error,
  onClose,
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
      error={error}
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
            {translate("devices.threadAccess")}<select
              className="relay-input mt-2 min-h-11 w-full"
              onChange={(event) =>
                setThreadAccess(event.target.value as RelayThreadAccessDto)
              }
              value={threadAccess}
            >
              <option value="read">{translate("devices.viewOnly")}</option>
              <option value="control">{translate("devices.collaborator")}</option>
            </select>
          </label>
          <label className="block text-sm text-[var(--theme-fg-soft)]">
            {translate("devices.workspaceAccess")}<select
              className="relay-input mt-2 min-h-11 w-full"
              disabled={workspaceAccessLocked}
              onChange={(event) =>
                setWorkspaceAccess(
                  event.target.value as RelayWorkspaceAccessDto,
                )
              }
              value={workspaceAccessLocked ? 'none' : workspaceAccess}
            >
              <option value="none">{translate("devices.noWorkspace")}</option>
              <option value="read">{translate("devices.workspaceRead")}</option>
              <option value="write">{translate("devices.workspaceWrite")}</option>
            </select>
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
          <button
            className="relay-button-primary min-h-11 w-full sm:w-auto"
            disabled={busy}
            type="submit"
          >
            {busy ? translate("devices.saving") : translate("devices.savePermissions")}
          </button>
        </div>
        </fieldset>
      </form>
    </RelayDialog>
  );
}

function ShareDeviceDialog({
  busy,
  device,
  error,
  onClose,
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
      error={error}
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
              {translate("devices.threadAccess")}<select
                className="relay-input mt-2 min-h-11 w-full"
                onChange={(event) =>
                  setThreadAccess(event.target.value as RelayThreadAccessDto)
                }
                value={threadAccess}
              >
                <option value="read">{translate("devices.viewOnly")}</option>
                <option value="control">{translate("devices.collaborator")}</option>
              </select>
            </label>
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.workspaceAccess")}<select
                className="relay-input mt-2 min-h-11 w-full"
                onChange={(event) =>
                  setWorkspaceAccess(
                    event.target.value as RelayWorkspaceAccessDto,
                  )
                }
                value={workspaceAccess}
              >
                <option value="none">{translate("devices.noWorkspace")}</option>
                <option value="read">{translate("devices.workspaceRead")}</option>
                <option value="write">{translate("devices.workspaceWrite")}</option>
              </select>
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
          <button
            className="relay-button-primary inline-flex min-h-11 w-full items-center justify-center gap-2 sm:w-auto"
            disabled={busy || !targetIdentifier.trim()}
            type="submit"
          >
            <Share2 className="h-4 w-4" />
            {busy ? translate("devices.sharing") : translate("devices.shareDevice")}
          </button>
        </div>
        </fieldset>
      </form>
    </RelayDialog>
  );
}

function GrantPermissionsDialog({
  busy,
  error,
  grant,
  onClose,
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
      error={error}
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
              {translate("devices.threadAccess")}<select
                className="relay-input mt-2 min-h-11 w-full"
                onChange={(event) =>
                  setThreadAccess(event.target.value as RelayThreadAccessDto)
                }
                value={threadAccess}
              >
                <option value="read">{translate("devices.viewOnly")}</option>
                <option value="control">{translate("devices.collaborator")}</option>
              </select>
            </label>
            <label className="block text-sm text-[var(--theme-fg-soft)]">
              {translate("devices.workspaceAccess")}<select
                className="relay-input mt-2 min-h-11 w-full"
                disabled={workspaceAccessLocked}
                onChange={(event) =>
                  setWorkspaceAccess(
                    event.target.value as RelayWorkspaceAccessDto,
                  )
                }
                value={workspaceAccessLocked ? 'none' : workspaceAccess}
              >
                <option value="none">{translate("devices.noWorkspace")}</option>
                <option value="read">{translate("devices.workspaceRead")}</option>
                <option value="write">{translate("devices.workspaceWrite")}</option>
              </select>
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
          <button
            className="relay-button-primary min-h-11 w-full sm:w-auto"
            disabled={busy}
            type="submit"
          >
            {busy ? translate("devices.saving") : translate("devices.savePermissions")}
          </button>
        </div>
        </fieldset>
      </form>
    </RelayDialog>
  );
}

function DeviceRow({
  device,
  busy,
  copiedSetup,
  copyError,
  onConnect,
  onCopySetup,
  onRotate,
  onDelete,
  onShare,
}: {
  device: RelayDeviceDto;
  busy: boolean;
  copiedSetup: boolean;
  copyError: string | null;
  onConnect: () => void;
  onCopySetup: (platform: SupervisorPlatform) => void;
  onRotate: () => void;
  onDelete: () => void;
  onShare: () => void;
}) {
  const { locale: i18nLocale } = useI18n();
  const [protectOpen, setProtectOpen] = useState(false);
  const [transportStatus, setTransportStatus] = useState(() =>
    getTransportStatus(device.id),
  );
  const protection = useDeviceProtection(device.id, transportStatus);
  useEffect(() => {
    const update = (event: Event) => {
      const status = (event as CustomEvent<NonNullable<typeof transportStatus>>).detail;
      if (status.deviceId === device.id) setTransportStatus(status);
    };
    window.addEventListener('remote-codex-transport', update);
    return () => window.removeEventListener('remote-codex-transport', update);
  }, [device.id]);
  const [actionsMenuOpen, setActionsMenuOpen] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement>(null);
  const actionsTriggerRef = useRef<HTMLButtonElement>(null);
  const menuFocusDirectionRef = useRef<'first' | 'last'>('first');
  const menuId = `device-actions-${useId()}`;
  const activityText = device.connected
    ? device.connectedAt
      ? `Online since ${formatRelayTimestamp(device.connectedAt)}`
      : translate("devices.onlineConnectedTimeUnavailable")
    : device.lastHeartbeatAt
      ? `Last heartbeat ${formatRelayTimestamp(device.lastHeartbeatAt)}`
      : translate("devices.noHeartbeatRecorded");

  useEffect(() => {
    if (!actionsMenuOpen) return;

    const focusFrame = window.requestAnimationFrame(() => {
      const items = Array.from(
        actionsMenuRef.current?.querySelectorAll<HTMLButtonElement>(
          '[role="menuitem"]:not(:disabled)',
        ) ?? [],
      );
      const target =
        menuFocusDirectionRef.current === 'last'
          ? items[items.length - 1]
          : items[0];
      target?.focus();
    });

    function closeOnOutsideClick(event: MouseEvent) {
      if (!actionsMenuRef.current?.contains(event.target as Node)) {
        setActionsMenuOpen(false);
      }
    }

    function closeOnEscape(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        setActionsMenuOpen(false);
        actionsTriggerRef.current?.focus();
      }
    }

    document.addEventListener('mousedown', closeOnOutsideClick);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener('mousedown', closeOnOutsideClick);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [actionsMenuOpen]);

  function openActionsMenu(direction: 'first' | 'last' = 'first') {
    menuFocusDirectionRef.current = direction;
    setActionsMenuOpen(true);
  }

  function handleMenuKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      if (event.key === 'Tab') {
        setActionsMenuOpen(false);
      }
      return;
    }
    const items = Array.from(
      actionsMenuRef.current?.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)',
      ) ?? [],
    );
    if (!items.length) {
      return;
    }
    event.preventDefault();
    const currentIndex = items.findIndex(
      (item) => item === document.activeElement,
    );
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? items.length - 1
          : event.key === 'ArrowUp'
            ? (currentIndex - 1 + items.length) % items.length
            : (currentIndex + 1) % items.length;
    items[nextIndex]?.focus();
  }

  function copySetup(platform: SupervisorPlatform) {
    actionsTriggerRef.current?.focus();
    setActionsMenuOpen(false);
    onCopySetup(platform);
  }

  return (
    <article className="grid min-w-0 gap-3 px-3 py-4 transition hover:bg-[var(--theme-hover)] md:grid-cols-[minmax(14rem,1fr)_minmax(14rem,0.8fr)_auto] md:items-center md:gap-5">
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <span
            aria-hidden="true"
            className={`device-presence-dot ${device.connected ? 'is-online' : 'is-offline'}`}
          />
          <p className="min-w-0 flex-1 truncate text-sm font-medium text-[var(--theme-fg)]">
            {device.name}
          </p>
          <span className={`device-presence-label ${device.connected ? 'is-online' : 'is-offline'}`}>
            {device.connected ? translate("devices.online_c3e839") : translate("devices.offline")}
          </span>
        </div>
        <p className="mt-1 truncate font-mono text-xs text-[var(--theme-fg-muted)]">
          {device.tokenPreview}
        </p>
      </div>
      <div className="min-w-0 text-xs text-[var(--theme-fg-muted)]">
        <p>{activityText}</p>
        <DeviceEncryptionStatus deviceId={device.id} online={device.connected} />
        <p role={protection === 'certificate-missing' ? 'alert' : undefined}>
          {translate(protectionLabel(protection))}
        </p>
        {(protection === 'not-protected' || protection === 'certificate-missing') && (
          <button
            className="relay-button-secondary mt-2 px-3 py-2"
            onClick={() => setProtectOpen(true)}
          >
            {translate(protection === 'certificate-missing' ? 'devices.repairDevice' : 'devices.protectDevice')}
          </button>
        )}
        {protectOpen && (
          <FormDialog
            title={translate(protection === 'certificate-missing' ? 'devices.repairDevice' : 'devices.protectDevice')}
            onClose={() => setProtectOpen(false)}
          >
            <DeviceBindingPanel
              deviceId={device.id}
              repair={protection === 'certificate-missing'}
            />
          </FormDialog>
        )}

        {copiedSetup ? (
          <p
            className="mt-1 text-[var(--status-success-fg)]"
            role="status"
          >
            {translate("devices.setupCommandCopied")}</p>
        ) : null}
      </div>
      <div className="flex min-w-0 items-center gap-2 md:justify-end">
        <button
          className="relay-button-primary inline-flex min-h-11 grow items-center justify-center gap-2 whitespace-nowrap md:grow-0 sm:min-h-10"
          disabled={!device.connected}
          onClick={onConnect}
          type="button"
        >
          <Plug className="h-4 w-4" />
          {translate("devices.connect")}
        </button>
        <div className="relative" ref={actionsMenuRef}>
          <button
            aria-controls={menuId}
            aria-expanded={actionsMenuOpen}
            aria-haspopup="menu"
            aria-label={translate("devices.moreActionsFor", { value1: device.name })}
            className="product-icon-button"
            onClick={() => {
              if (actionsMenuOpen) {
                setActionsMenuOpen(false);
              } else {
                openActionsMenu();
              }
            }}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault();
                openActionsMenu(
                  event.key === 'ArrowUp' ? 'last' : 'first',
                );
              }
            }}
            ref={actionsTriggerRef}
            type="button"
          >
            <Ellipsis className="h-5 w-5" />
          </button>
          {actionsMenuOpen ? (
            <div
              aria-label={translate("devices.actionsFor", { value1: device.name })}
              className="absolute right-0 top-full z-40 mt-1 w-64 max-w-[calc(100vw-2rem)] rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-1 shadow-[var(--theme-shadow)]"
              id={menuId}
              onKeyDown={handleMenuKeyDown}
              role="menu"
            >
              <button
                className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] disabled:cursor-not-allowed disabled:opacity-50"
                onClick={() => copySetup('unix')}
                role="menuitem"
                type="button"
              >
                <Copy className="h-4 w-4" />
                {translate("devices.copySetupForMacOSLinux")}</button>
              <button
                className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] disabled:cursor-not-allowed disabled:opacity-50"
                onClick={() => copySetup('windows')}
                role="menuitem"
                type="button"
              >
                <Copy className="h-4 w-4" />
                {translate("devices.copySetupForWindows")}</button>
              <button
                className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-[var(--theme-fg)] transition hover:bg-[var(--theme-hover)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)]"
                onClick={() => {
                  actionsTriggerRef.current?.focus();
                  setActionsMenuOpen(false);
                  onShare();
                }}
                role="menuitem"
                type="button"
              >
                <Share2 className="h-4 w-4" />
                {translate("devices.shareDevice")}</button>
              <button className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm hover:bg-[var(--theme-hover)]" role="menuitem" onClick={()=>{setActionsMenuOpen(false);onRotate();}}>{translate("devices.replaceDeviceToken_268410")}</button>
              <div className="mt-1 border-t border-[var(--theme-border)] pt-1">
                <button
                  className="flex min-h-11 w-full items-center gap-2 rounded-md px-3 py-2 text-left text-sm text-[var(--status-danger-fg)] transition hover:bg-[var(--status-danger-bg)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] disabled:cursor-not-allowed disabled:opacity-50"
                  disabled={busy}
                  onClick={() => {
                    actionsTriggerRef.current?.focus();
                    setActionsMenuOpen(false);
                    onDelete();
                  }}
                  role="menuitem"
                  title={translate("devices.delete_43de5b", { value1: device.name })}
                  type="button"
                >
                  <Trash2 className="h-4 w-4" />
                  {translate("devices.removeDevice")}</button>
              </div>
            </div>
          ) : null}
        </div>
      </div>
      {copyError ? (
        <p
          className="text-sm text-[var(--status-danger-fg)] md:col-span-3"
          role="alert"
        >
          {copyError}
        </p>
      ) : null}
    </article>
  );
}

function DeviceTokenPanel({ result, binding }: {
  result: RelayCreateDeviceResultDto;
  binding: DeviceBindingFlags;
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
      className={`min-h-11 rounded-md px-2.5 text-xs font-medium transition focus:outline-none focus:ring-2 focus:ring-[var(--theme-accent-ring)] sm:min-h-9 ${
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
        <button
          aria-label={copyLabel}
          className="relay-button-secondary inline-flex min-h-11 items-center gap-1 px-2 text-xs sm:min-h-9"
          onClick={() => void copy()}
          type="button"
        >
          <Copy className="h-3.5 w-3.5" />
          {copyState === 'copied' ? translate("devices.copied") : translate("devices.copy")}
        </button>
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
      {copyState === 'error' ? (
        <p
          className="mt-1 text-xs text-[var(--status-danger-fg)]"
          role="alert"
        >
          {translate("devices.clipboardAccessFailedSelectTheTextAnd")}</p>
      ) : null}
    </div>
  );
}

function Notice({ children }: {
  tone: 'danger';
  children: React.ReactNode;
}) {
  const { locale: i18nLocale } = useI18n();
  return (
    <div
      className="rounded-lg border border-[var(--status-danger-border)] bg-[var(--status-danger-bg)] px-3 py-2 text-sm text-[var(--status-danger-fg)]"
      role="alert"
    >
      {children}
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
