// Settings: registration, sign-in providers, data retention and privacy, and
// the admin's own account security.
import { ErrorMark } from '@pockymoe/thread-ui';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Check, Eye, EyeOff, Lock } from 'lucide-react';
import { useEffect, useId, useState, type FormEvent, type ReactNode } from 'react';
import type { RelayRegistrationSettingsDto, RelaySessionDto } from '@pockymoe/shared';
import { RelaySecurityPanel } from '../../components/RelaySecurity';
import { updateRelayRegistrationSettings } from '../../lib/api';
import { adminApi } from './adminApi';
import type { AdminPageProps } from './RelayAdminConsole';
import type { AdminRetention } from './types';
import { Badge, Card, errorText, LoadFailure, Skeleton, useAdminResource } from './ui';

export function SettingsPage(props: AdminPageProps & { session: RelaySessionDto | null }) {
  useI18n();
  const registration = useAdminResource(() => adminApi.registrationSettings(), `registration:${props.refreshKey}`, props.onUnauthorized);
  const loaded = props.onLoaded;
  useEffect(() => {
    if (registration.data) loaded();
  }, [registration.data, loaded]);
  return (
    <div className="admin-page">
      <div className="admin-grid is-halves is-top">
        {registration.data ? (
          <RegistrationSettings
            onSaved={(settings) => registration.setData({ registrationEnabled: settings.enabled, settings })}
            settings={registration.data.settings}
          />
        ) : registration.error ? <LoadFailure message={registration.error} onRetry={registration.reload} /> : <Skeleton lines={5} />}
        {registration.data ? (
          <ProviderSettings
            onSaved={(settings) => registration.setData({ registrationEnabled: settings.enabled, settings })}
            settings={registration.data.settings}
          />
        ) : null}
        <RetentionSettings {...props} />
      </div>
      <Card subtitle={translate('admin.adminAccountSubtitle', { name: props.session?.user?.username ?? '' })} title={translate('admin.adminAccount')}>
        <div className="admin-embedded-security">
          <RelaySecurityPanel realm="relay-admin" />
        </div>
      </Card>
    </div>
  );
}

type Saving = { key: string; state: 'saving' | 'saved' | 'failed'; message?: string } | null;

/** Saves one setting at once and shows "Saved" beside it (docs/ui-principles.md §15). */
function useSettingSaver(onSaved: (settings: RelayRegistrationSettingsDto) => void) {
  const [saving, setSaving] = useState<Saving>(null);
  const save = async (key: string, input: Partial<RelayRegistrationSettingsDto>) => {
    setSaving({ key, state: 'saving' });
    try {
      const result = await updateRelayRegistrationSettings(input);
      onSaved(result.settings);
      setSaving({ key, state: 'saved' });
      window.setTimeout(() => setSaving((current) => (current?.key === key && current.state === 'saved' ? null : current)), 2000);
      return true;
    } catch (caught) {
      setSaving({ key, state: 'failed', message: errorText(caught) });
      return false;
    }
  };
  return { saving, save };
}

function SaveState({ saving, at }: { saving: Saving; at: string }) {
  if (saving?.key !== at) return null;
  if (saving.state === 'saved') return <span className="admin-saved" role="status"><Check aria-hidden className="h-4 w-4" />{translate('admin.saved')}</span>;
  if (saving.state === 'failed') return <ErrorMark message={saving.message ?? ''} />;
  return <span className="admin-saved is-saving" role="status">{translate('admin.saving')}</span>;
}

function SettingRow({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <div className="admin-setting" role="group" aria-labelledby={id}>
      <div className="admin-setting-text">
        <b id={id}>{title}</b>
        {description ? <span>{description}</span> : null}
      </div>
      <div className="admin-setting-control">{children}</div>
    </div>
  );
}

function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button aria-checked={checked} aria-label={label} className="admin-switch" disabled={disabled} onClick={() => onChange(!checked)} role="switch" type="button">
      <span aria-hidden />
    </button>
  );
}

function RegistrationSettings({ settings, onSaved }: { settings: RelayRegistrationSettingsDto; onSaved: (settings: RelayRegistrationSettingsDto) => void }) {
  useI18n();
  const { saving, save } = useSettingSaver(onSaved);
  const [code, setCode] = useState('');
  const [visible, setVisible] = useState(false);
  const busy = saving?.state === 'saving';
  const submitCode = async (event: FormEvent) => {
    event.preventDefault();
    if (code.trim().length < 8) return;
    if (await save('code', { registrationPassword: code.trim() })) setCode('');
  };
  return (
    <Card subtitle={translate('admin.registrationSubtitle')} title={translate('admin.registration')}>
      <SettingRow description={translate('admin.openRegistrationHelp')} title={translate('admin.openRegistration')}>
        <SaveState at="enabled" saving={saving} />
        <Toggle checked={settings.enabled} disabled={busy} label={translate('admin.openRegistration')} onChange={(enabled) => void save('enabled', { enabled })} />
      </SettingRow>
      <SettingRow description={translate('admin.requireApprovalHelp')} title={translate('admin.requireApproval')}>
        <SaveState at="approval" saving={saving} />
        <Toggle checked={settings.approvalRequired} disabled={busy} label={translate('admin.requireApproval')} onChange={(approvalRequired) => void save('approval', { approvalRequired })} />
      </SettingRow>
      <form className="admin-setting is-stacked" onSubmit={submitCode}>
        <div className="admin-setting-text">
          <b>{translate('admin.inviteCode')}</b>
          <span>{settings.registrationPasswordConfigured ? translate('admin.inviteCodeSet') : translate('admin.inviteCodeNone')}</span>
        </div>
        <div className="admin-invite">
          <input
            aria-label={translate('admin.newInviteCode')}
            autoComplete="off"
            className="relay-input"
            minLength={8}
            onChange={(event) => setCode(event.target.value)}
            placeholder={translate('admin.newInviteCode')}
            type={visible ? 'text' : 'password'}
            value={code}
          />
          <button aria-label={visible ? translate('admin.hideCode') : translate('admin.showCode')} aria-pressed={visible} className="admin-icon-button is-bordered" onClick={() => setVisible((value) => !value)} type="button">
            {visible ? <EyeOff aria-hidden className="h-4 w-4" /> : <Eye aria-hidden className="h-4 w-4" />}
          </button>
          <button className="relay-button-primary" disabled={busy || code.trim().length < 8} type="submit">{translate('admin.save')}</button>
          {settings.registrationPasswordConfigured ? (
            <button className="relay-button-secondary" disabled={busy} onClick={() => void save('code', { registrationPassword: null })} type="button">{translate('admin.removeCode')}</button>
          ) : null}
          <SaveState at="code" saving={saving} />
        </div>
        {code && code.trim().length < 8 ? <p className="admin-field-hint">{translate('admin.inviteCodeTooShort')}</p> : null}
      </form>
    </Card>
  );
}

function ProviderSettings({ settings, onSaved }: { settings: RelayRegistrationSettingsDto; onSaved: (settings: RelayRegistrationSettingsDto) => void }) {
  useI18n();
  const { saving, save } = useSettingSaver(onSaved);
  const busy = saving?.state === 'saving';
  const row = (key: 'github' | 'google', title: string, available: boolean, enabled: boolean, field: 'githubAuthEnabled' | 'googleAuthEnabled') => (
    <SettingRow description={available ? translate('admin.providerReady') : translate('admin.providerMissing')} title={title}>
      <SaveState at={key} saving={saving} />
      {!available ? <Badge tone="neutral">{translate('admin.notConfigured')}</Badge> : null}
      <Toggle checked={enabled && available} disabled={busy || !available} label={title} onChange={(value) => void save(key, { [field]: value })} />
    </SettingRow>
  );
  return (
    <Card title={translate('admin.loginMethods')}>
      {row('github', 'GitHub', settings.githubAuthAvailable, settings.githubAuthEnabled, 'githubAuthEnabled')}
      {row('google', 'Google', settings.googleAuthAvailable, settings.googleAuthEnabled, 'googleAuthEnabled')}
      <SettingRow description={translate('admin.emailVerificationHelp')} title={translate('admin.emailVerification')}>
        <Badge tone="neutral">{translate('admin.unavailable')}</Badge>
        <Toggle checked={false} disabled label={translate('admin.emailVerification')} onChange={() => undefined} />
      </SettingRow>
    </Card>
  );
}

function RetentionSettings(props: AdminPageProps) {
  useI18n();
  const { data, error, reload, setData } = useAdminResource<AdminRetention | null>(
    () => adminApi.retention().catch(() => null),
    `retention:${props.refreshKey}`,
    props.onUnauthorized,
  );
  const [saving, setSaving] = useState<Saving>(null);
  const update = async (key: 'metricsDays' | 'securityDays', value: number) => {
    setSaving({ key, state: 'saving' });
    try {
      setData(await adminApi.updateRetention({ [key]: value }));
      setSaving({ key, state: 'saved' });
      window.setTimeout(() => setSaving((current) => (current?.key === key && current.state === 'saved' ? null : current)), 2000);
    } catch (caught) {
      setSaving({ key, state: 'failed', message: errorText(caught) });
    }
  };
  return (
    <Card subtitle={translate('admin.privacySubtitle')} title={translate('admin.retentionAndPrivacy')}>
      {error ? <LoadFailure message={error} onRetry={reload} /> : null}
      {data ? (
        <>
          <SettingRow description={translate('admin.metricsRetentionHelp')} title={translate('admin.metricsRetention')}>
            <SaveState at="metricsDays" saving={saving} />
            <Choice options={data.options.metricsDays} value={data.metricsDays} label={translate('admin.metricsRetention')} onChange={(value) => void update('metricsDays', value)} />
          </SettingRow>
          <SettingRow description={translate('admin.securityRetentionHelp', { max: data.fixed.securityMaxEvents })} title={translate('admin.securityRetention')}>
            <SaveState at="securityDays" saving={saving} />
            <Choice options={data.options.securityDays} value={data.securityDays} label={translate('admin.securityRetention')} onChange={(value) => void update('securityDays', value)} />
          </SettingRow>
          <SettingRow description={translate('admin.fixedRetentionHelp', { connections: data.fixed.connectionDays, actions: data.fixed.adminActionDays })} title={translate('admin.fixedRetention')}>
            <span />
          </SettingRow>
        </>
      ) : !error ? null : null}
      {data === null && !error ? <p className="admin-empty">{translate('admin.retentionUnavailable')}</p> : null}
      <p className="admin-privacy-note">
        <Lock aria-hidden className="h-4 w-4" />
        {translate('admin.privacyStatement')}
      </p>
    </Card>
  );
}

function Choice({ options, value, onChange, label }: { options: number[]; value: number; onChange: (value: number) => void; label: string }) {
  return (
    <div aria-label={label} className="admin-range" role="group">
      {options.map((option) => (
        <button aria-pressed={option === value} key={option} onClick={() => onChange(option)} type="button">
          {option >= 365 && option % 365 === 0 ? translate('admin.years', { count: option / 365 }) : translate('admin.days', { count: option })}
        </button>
      ))}
    </div>
  );
}
