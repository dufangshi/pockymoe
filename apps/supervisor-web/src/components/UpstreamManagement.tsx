// Provider list/search/actions adapted from CC Switch src/components/providers.
// Copyright (c) 2025 Jason Young. MIT; see THIRD_PARTY_NOTICES.md.
import { getLocale } from '@pockymoe/thread-ui/i18n';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useEffect, useRef, useState } from 'react';
import {
  Plus,
  Upload,
  Download,
  Check,
  FlaskConical,
  RotateCcw,
  Copy,
  Trash2,
} from 'lucide-react';
import { request } from '../lib/api';
import { FormDialog } from './FormDialog';
import { UpstreamProviderCard } from './UpstreamProviderCard';
import { UpstreamProviderForm } from './UpstreamProviderForm';
import { UpstreamModelPicker } from './UpstreamModelPicker';

export type Profile = {
  settingsConfig?: Record<string, unknown>;
  revision?: string | undefined;
  liveRevision?: string | undefined;
  sortIndex?: number;
  id: string;
  name: string;
  harness: string;
  baseUrl: string;
  model: string;
  apiType: string;
  authType?: string;
  contextWindow: number;
  hasApiKey?: boolean;
  apiKey?: string;
};
type Snapshot = {
  profiles: Profile[];
  active: Record<string, string>;
  backups: { id: string; harness: string; createdAt: string }[];
};
type Job = { state: string; error?: string; completed?: string[] };
const labels: Record<string, string> = {
  codex: 'OpenAI Codex',
  claude: 'Claude Agent',
  gemini: 'Gemini CLI',
  grok: 'Grok Build',
  deepseek: 'DeepSeek Harness',
};
const button =
  'inline-flex min-h-9 items-center justify-center gap-1.5 rounded-md border border-[var(--theme-border)] px-3 py-1.5 text-xs hover:bg-[var(--theme-hover)] disabled:opacity-40';
const field =
  'host-input mt-1 w-full rounded-md border border-[var(--theme-border)] bg-[var(--theme-panel)] p-2 text-sm';
const blank = (): Profile => ({
  id: '',
  name: '',
  harness: 'codex',
  baseUrl: '',
  model: '',
  apiKey: '',
  apiType: 'responses',
  authType: 'api_key',
  contextWindow: 500000,
});
function snapshot(value: Snapshot) {
  if (
    !Array.isArray(value?.profiles) ||
    !Array.isArray(value?.backups) ||
    !value?.active
  )
    throw Error(translate('settings.updateThisSupervisorToEnableUpstreamAnd'));
  return value;
}
const sample = {
  schemaVersion: 1,
  harnesses: ['codex'],
  profiles: [
    {
      name: 'My upstream',
      harness: 'codex',
      baseUrl: 'https://example.com/v1',
      apiKey: '',
      apiType: 'responses',
    },
  ],
};
export function UpstreamManagement({
  apiRoot,
  harness,
  templatesOnly = false,
  appearance = 'standard',
  harnessName,
  installed = true,
  onChanged,
}: {
  apiRoot: string;
  harness?: string;
  templatesOnly?: boolean;
  /** `advanced` keeps only native-config tools; connections are edited elsewhere. */
  appearance?: 'standard' | 'switch' | 'advanced';
  harnessName?: string | undefined;
  installed?: boolean;
  onChanged?: () => void;
}) {
  useI18n();
  const [search, setSearch] = useState('');
  const [editConflict, setEditConflict] = useState(false);
  const harnessOptions = Object.entries(labels).filter(
    ([id]) => !harness || id === harness,
  );
  const [deleting, setDeleting] = useState<Profile | null>(null);
  const [data, setData] = useState<Snapshot>({
    profiles: [],
    active: {},
    backups: [],
  });
  const [loaded, setLoaded] = useState(false),
    [error, setError] = useState(''),
    [notice, setNotice] = useState(''),
    [busy, setBusy] = useState(false);
  const [editor, setEditor] = useState<Profile | null>(null),
    [mode, setMode] = useState<'config' | 'template' | null>(null);
  const [text, setText] = useState(''),
    [importHarness, setImportHarness] = useState(harness ?? 'codex'),
    [importName, setImportName] = useState('Imported upstream'),
    [importKey, setImportKey] = useState('');
  const [preview, setPreview] = useState<{
      harnesses: string[];
      profiles: Profile[];
    } | null>(null),
    [job, setJob] = useState<Job | null>(null);
  const file = useRef<HTMLInputElement>(null);
  const api = <T,>(suffix: string, body?: unknown, method?: string) =>
    request<T>(
      `${apiRoot}/management/${suffix}`,
      body !== undefined
        ? { method: method ?? 'POST', body: JSON.stringify(body) }
        : { method: method ?? 'GET', cache: 'no-store' },
    );
  async function load() {
    const value = snapshot(await api<Snapshot>('upstreams'));
    setData(value);
    setLoaded(true);
    onChanged?.();
  }
  useEffect(() => {
    let alive = true;
    void api<Record<string, Job>>('jobs')
      .then((jobs) => {
        if (alive && jobs.template) setJob(jobs.template);
      })
      .catch(() => {});
    void api<Snapshot>('upstreams')
      .then(snapshot)
      .then((v) => {
        if (alive) {
          setData(v);
          setLoaded(true);
        }
      })
      .catch(() => {
        if (alive)
          setError(
            translate('settings.updateThisSupervisorToEnableUpstreamAnd'),
          );
      });
    return () => {
      alive = false;
    };
  }, [apiRoot]);
  useEffect(() => {
    if (job?.state !== 'running') return;
    let alive = true,
      inFlight = false;
    const timer = window.setInterval(async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const jobs = await api<Record<string, Job>>('jobs');
        if (alive && jobs.template) {
          setJob(jobs.template);
          if (jobs.template.state !== 'running') await load();
        }
      } catch {
      } finally {
        inFlight = false;
      }
    }, 2000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [job?.state, apiRoot]);
  async function perform(action: () => Promise<void>) {
    setBusy(true);
    setError('');
    setNotice('');
    try {
      await action();
    } catch (e) {
      if (
        (e as { payload?: { code?: string } })?.payload?.code ===
        'edit_conflict'
      )
        setEditConflict(true);
      setError(
        e instanceof Error
          ? e.message
          : translate('settings.unableToSaveUpstreamSettings'),
      );
    } finally {
      setBusy(false);
    }
  }
  // Search adapted from CC Switch ProviderList.tsx (MIT, Jason Young).
  const keyword = search.trim().toLowerCase();
  const visibleProfiles = data.profiles.filter(
    (p) =>
      (!harness || p.harness === harness) &&
      (!keyword ||
        [p.name, p.baseUrl, p.model].some((v) =>
          v.toLowerCase().includes(keyword),
        )),
  );
  async function edit(p: Profile) {
    await perform(async () => {
      const current = await api<Profile>(`upstreams/${p.id}`, {
        action: 'edit',
      });
      setEditor({ ...current, apiKey: '' });
    });
  }
  function duplicate(p: Profile) {
    setError('');
    setEditor({
      ...p,
      id: '',
      revision: undefined,
      liveRevision: undefined,
      apiKey: '',
      hasApiKey: false,
      settingsConfig: JSON.parse(
        JSON.stringify(p.settingsConfig ?? {}),
        (_key, value) => (value === '[stored privately]' ? undefined : value),
      ),
    });
  }
  const disabled = busy || job?.state === 'running';
  function download() {
    const template = {
      schemaVersion: 1,
      harnesses: [
        ...new Set(
          data.profiles
            .filter((p) => data.active[p.harness] === p.id)
            .map((p) => p.harness),
        ),
      ],
      profiles: data.profiles
        .filter((p) => data.active[p.harness] === p.id)
        .map(({ id, hasApiKey, model, ...p }) => ({ ...p, apiKey: '' })),
    };
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(template, null, 2)], {
        type: 'application/json',
      }),
    );
    const a = document.createElement('a');
    a.href = url;
    a.download = 'pockymoe-template.json';
    a.click();
    URL.revokeObjectURL(url);
    setNotice(translate('settings.templateExportedWithoutAPIKeysFillThem'));
  }
  function openImport(next: 'config' | 'template') {
    setMode(next);
    setImportHarness(harness ?? 'codex');
    setText(next === 'template' ? JSON.stringify(sample, null, 2) : '');
    setPreview(null);
    setError('');
    setImportKey('');
  }
  const backupsView = !!data.backups.length && (
    <details className="mt-3 text-xs">
      <summary className="cursor-pointer py-2">
        {translate('settings.configurationBackups')}
      </summary>
      {Object.keys(labels)
        .filter((h) => !harness || h === harness)
        .map((h) => {
          const b = [...data.backups].reverse().find((v) => v.harness === h);
          return (
            b && (
              <div
                key={h}
                className="flex flex-wrap items-center justify-between gap-2 py-2"
              >
                <span>
                  {labels[h]} ·{' '}
                  {new Date(b.createdAt).toLocaleString(getLocale())}
                </span>
                <button
                  className={button}
                  disabled={disabled}
                  onClick={() =>
                    void perform(async () => {
                      await api(`upstreams/${b.id}`, {
                        action: 'restore',
                      });
                      await load();
                      setNotice(
                        translate(
                          'settings.previousConfigurationRestoredTheNextTurnReloads',
                        ),
                      );
                    })
                  }
                >
                  <RotateCcw size={13} />
                  {translate('settings.restorePrevious')}
                </button>
              </div>
            )
          );
        })}
    </details>
  );
  return (
    <section
      className={
        appearance === 'switch' || appearance === 'advanced'
          ? 'mt-3 min-w-0'
          : 'mt-6 border-t border-[var(--theme-border)] pt-5'
      }
      aria-label={
        templatesOnly
          ? translate('settings.deviceTemplates')
          : translate('settings.upstreamManagement')
      }
    >
      {appearance === 'advanced' && !templatesOnly && (
        <>
          <p className="settings-hint text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate('settings.connAdvancedNativeHint')}
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            <button
              className={button}
              disabled={!loaded || disabled}
              onClick={() => openImport('config')}
            >
              <Upload size={14} />
              {translate('settings.importConfig')}
            </button>
            <button
              className={button}
              disabled={!loaded || disabled || !installed}
              onClick={() =>
                void perform(async () => {
                  await api('upstreams/live', {
                    harness,
                    name: harnessName ?? labels[harness ?? 'codex'],
                  });
                  await load();
                  setNotice(translate('settings.upstreamsImportedLive'));
                })
              }
            >
              {translate('settings.upstreamsImportLive')}
            </button>
          </div>
          {backupsView}
        </>
      )}
      {!templatesOnly && appearance !== 'advanced' && (
        <>
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0 flex-1">
              <h3 className="text-sm font-semibold">
                {harness
                  ? translate('settings.upstreams', {
                      value1: harnessName ?? labels[harness],
                    })
                  : translate('settings.upstreams_8b39d6')}
              </h3>
              {appearance !== 'switch' && (
                <p className="settings-hint mt-1 text-xs text-[var(--theme-fg-muted)]">
                  {translate(
                    'settings.savedOnThisDeviceSwitchProvidersWithout',
                  )}
                </p>
              )}
            </div>
            <div className="shrink-0">
              <button
                className={button}
                disabled={!loaded || disabled}
                onClick={() =>
                  setEditor({
                    ...blank(),
                    harness: harness ?? 'codex',
                    apiType: harness === 'deepseek' ? 'anthropic' : 'responses',
                  })
                }
              >
                <Plus size={14} />
                {translate('settings.addUpstream')}
              </button>
            </div>
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            <button
              className={button}
              disabled={!loaded || disabled}
              onClick={() => openImport('config')}
            >
              <Upload size={14} />
              {translate('settings.importConfig')}
            </button>
            {appearance === 'switch' && (
              <button
                className={button}
                disabled={!loaded || disabled || !installed}
                onClick={() =>
                  void perform(async () => {
                    await api('upstreams/live', {
                      harness,
                      name: harnessName ?? labels[harness ?? 'codex'],
                    });
                    await load();
                    setNotice(translate('settings.upstreamsImportedLive'));
                  })
                }
              >
                {translate('settings.upstreamsImportLive')}
              </button>
            )}
          </div>
          {loaded &&
            !data.profiles.some((p) => !harness || p.harness === harness) && (
              <p className="my-5 rounded-lg border border-dashed border-[var(--theme-border)] p-4 text-sm text-[var(--theme-fg-muted)]">
                {translate('settings.addAnAPIProviderOrImportAn')}
              </p>
            )}
          {appearance === 'switch' && (
            <>
              <input
                type="search"
                className={`${field} my-4`}
                aria-label={translate('settings.upstreamsSearch')}
                placeholder={translate('settings.upstreamsSearch')}
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              <div className="mb-4 flex flex-wrap items-center gap-3 text-xs text-[var(--theme-fg-muted)]">
                <label className="flex items-center gap-2">
                  {translate('settings.upstreamsMode')}
                  <select
                    className="host-input rounded-lg border border-[var(--theme-border)] p-2"
                    aria-label={translate('settings.upstreamsMode')}
                    value="direct"
                    disabled
                  >
                    <option value="direct">
                      {translate('settings.upstreamsDirect')}
                    </option>
                  </select>
                </label>
                <span className="settings-hint hidden sm:inline">
                  {translate('settings.upstreamsDirectHint')}
                </span>
              </div>
              {keyword && !visibleProfiles.length && (
                <p className="py-4 text-sm text-[var(--theme-fg-muted)]">
                  {translate('settings.upstreamsNoMatches')}
                </p>
              )}
            </>
          )}
          <div className="mt-3 space-y-3">
            {visibleProfiles.map((p) =>
              appearance === 'switch' ? (
                <UpstreamProviderCard
                  key={p.id}
                  name={p.name}
                  baseUrl={p.baseUrl}
                  model={p.model}
                  active={data.active[p.harness] === p.id}
                  disabled={disabled}
                  canActivate={installed}
                  canMoveUp={
                    data.profiles.filter((v) => v.harness === p.harness)[0]
                      ?.id !== p.id
                  }
                  canMoveDown={
                    data.profiles.filter((v) => v.harness === p.harness).at(-1)
                      ?.id !== p.id
                  }
                  onMove={(direction) =>
                    void perform(async () => {
                      await api(`upstreams/${p.id}`, { action: direction });
                      await load();
                    })
                  }
                  onSpeed={() =>
                    void perform(async () => {
                      const r = await api<{
                        latencyMs: number;
                        status: number;
                      }>(`upstreams/${p.id}`, { action: 'speed' });
                      setNotice(
                        translate('settings.upstreamsSpeedResult', {
                          name: p.name,
                          latency: r.latencyMs,
                          status: r.status,
                        }),
                      );
                    })
                  }
                  onActivate={() =>
                    void perform(async () => {
                      await api(`upstreams/${p.id}`, { action: 'activate' });
                      await load();
                      setNotice(
                        translate(
                          'settings.configurationAppliedIdleSessionsWereRestartedThe',
                          { value1: labels[p.harness] },
                        ),
                      );
                    })
                  }
                  onEdit={() => void edit(p)}
                  onDuplicate={() => duplicate(p)}
                  onTest={() =>
                    void perform(async () => {
                      const r = await api<{ latencyMs: number }>(
                        `upstreams/${p.id}`,
                        { action: 'test' },
                      );
                      setNotice(
                        translate('settings.connectionSucceededMs', {
                          value1: p.name,
                          value2: r.latencyMs,
                        }),
                      );
                    })
                  }
                  onDelete={() => setDeleting(p)}
                />
              ) : (
                <article
                  key={p.id}
                  className="rounded-lg border border-[var(--theme-border)] bg-[var(--theme-panel)] p-3"
                >
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <h4 className="flex items-center gap-2 text-sm font-medium">
                        {p.name}
                        {data.active[p.harness] === p.id && (
                          <span className="inline-flex items-center gap-1 text-xs text-[var(--theme-accent-strong)]">
                            <Check size={13} />
                            {translate('settings.active')}
                          </span>
                        )}
                      </h4>
                      <p className="mt-1 break-all text-xs text-[var(--theme-fg-muted)]">
                        {labels[p.harness]} · {p.model}
                      </p>
                      <p className="mt-1 break-all text-xs text-[var(--theme-fg-muted)]">
                        {p.baseUrl}
                      </p>
                    </div>
                    <button
                      className={button}
                      disabled={disabled}
                      aria-label={translate('settings.duplicate', {
                        value1: p.name,
                      })}
                      onClick={() => setEditor({ ...p, id: '', apiKey: '' })}
                    >
                      <Copy size={13} />
                    </button>
                  </div>
                  <div className="mt-3 flex flex-wrap gap-2">
                    <button
                      className={button}
                      disabled={disabled || data.active[p.harness] === p.id}
                      onClick={() =>
                        void perform(async () => {
                          await api(`upstreams/${p.id}`, {
                            action: 'activate',
                          });
                          await load();
                          setNotice(
                            translate(
                              'settings.configurationAppliedIdleSessionsWereRestartedThe',
                              { value1: labels[p.harness] },
                            ),
                          );
                        })
                      }
                    >
                      {translate('settings.useUpstream')}
                    </button>
                    <button
                      className={button}
                      disabled={disabled}
                      title={translate(
                        'settings.sendsASmallRequestUsingThisModel',
                      )}
                      onClick={() =>
                        void perform(async () => {
                          const r = await api<{ latencyMs: number }>(
                            `upstreams/${p.id}`,
                            { action: 'test' },
                          );
                          setNotice(
                            translate('settings.connectionSucceededMs', {
                              value1: p.name,
                              value2: r.latencyMs,
                            }),
                          );
                        })
                      }
                    >
                      <FlaskConical size={13} />
                      {translate('settings.testConnection')}
                    </button>
                    <button
                      className={button}
                      disabled={disabled || data.active[p.harness] === p.id}
                      onClick={() => setEditor({ ...p, apiKey: '' })}
                    >
                      {translate('settings.edit')}
                    </button>
                    <button
                      className={button}
                      disabled={disabled}
                      aria-label={translate('settings.delete', {
                        value1: p.name,
                      })}
                      onClick={() => setDeleting(p)}
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </article>
              ),
            )}
          </div>
          <p className="settings-hint mt-3 text-xs leading-5 text-[var(--theme-fg-muted)]">
            {translate('settings.switchAfterCurrentTasksFinishExistingMCP')}
          </p>
          {backupsView}
        </>
      )}
      {templatesOnly && (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h3 className="text-sm font-semibold">
              {translate('settings.deviceTemplates')}
            </h3>
            <p className="settings-hint mt-1 text-xs text-[var(--theme-fg-muted)]">
              {translate(
                'settings.installHarnessesAndConfigureTheirUpstreamsTogether',
              )}
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <button
              className={button}
              disabled={!loaded || disabled}
              onClick={() => openImport('template')}
            >
              <Upload size={14} />
              {translate('settings.importTemplate')}
            </button>
            <button
              className={button}
              disabled={!loaded || disabled || !Object.keys(data.active).length}
              onClick={download}
            >
              <Download size={14} />
              {translate('settings.exportTemplate')}
            </button>
          </div>
        </div>
      )}
      {deleting && (
        <FormDialog
          title={
            data.active[deleting.harness] === deleting.id
              ? translate('settings.deactivateAndDeleteUpstream')
              : translate('settings.deleteUpstream')
          }
          onClose={() => setDeleting(null)}
          busy={busy}
        >
          <p className="text-sm leading-6">
            {data.active[deleting.harness] === deleting.id
              ? translate(
                  'settings.thisRestoresTheNativeConfigurationFromBefore',
                )
              : translate('settings.removeThisSavedUpstreamFromTheDevice')}
          </p>
          <p className="my-3 text-sm font-medium">{deleting.name}</p>
          {error && (
            <p
              role="alert"
              className="mb-3 text-xs text-[var(--status-danger-fg)]"
            >
              {error}
            </p>
          )}
          <button
            className="relay-button-primary min-h-10"
            disabled={busy}
            onClick={() =>
              void perform(async () => {
                await api(`upstreams/${deleting.id}`, undefined, 'DELETE');
                await load();
                setDeleting(null);
                setNotice(translate('settings.upstreamRemoved'));
              })
            }
          >
            {busy
              ? translate('settings.removing')
              : translate('settings.deleteUpstream')}
          </button>
        </FormDialog>
      )}
      {job && (
        <div role={job.error ? 'alert' : 'status'} className="mt-3 text-xs">
          <p>
            {job.state === 'running'
              ? translate('settings.installingAndConfiguringThisDevice')
              : (job.error ??
                translate('settings.templateAppliedThisDeviceIsReady'))}
          </p>
          {job.completed?.map((v) => (
            <p key={v}>{v}</p>
          ))}
        </div>
      )}
      {notice && (
        <p
          role="status"
          className="mt-3 text-xs text-[var(--theme-accent-strong)]"
        >
          {notice}
        </p>
      )}
      {error && !editor && !mode && (
        <p role="alert" className="mt-3 text-xs text-[var(--status-danger-fg)]">
          {error}
        </p>
      )}
      {editConflict && (
        <FormDialog
          title={translate('settings.upstreamsEditConflict')}
          onClose={() => setEditConflict(false)}
          busy={busy}
        >
          <p className="text-sm leading-6">
            {translate('settings.upstreamsEditConflictHint')}
          </p>
          <div className="mt-4 flex flex-wrap gap-2">
            <button className={button} onClick={() => setEditConflict(false)}>
              {translate('settings.upstreamsKeepEditing')}
            </button>
            <button
              className="relay-button-primary min-h-10"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  if (editor?.id) {
                    const current = await api<Profile>(
                      `upstreams/${editor.id}`,
                      { action: 'edit' },
                    );
                    setEditor({ ...current, apiKey: '' });
                  }
                  setEditConflict(false);
                })
              }
            >
              {translate('settings.upstreamsReloadEditor')}
            </button>
          </div>
        </FormDialog>
      )}
      {editor && !editConflict && (
        <FormDialog
          title={
            editor.id
              ? translate('settings.editUpstream')
              : translate('settings.addUpstream')
          }
          onClose={() => setEditor(null)}
          busy={busy}
        >
          {appearance === 'switch' ? (
            <UpstreamProviderForm
              key={
                editor.id +
                (editor.revision ?? '') +
                (editor.liveRevision ?? '')
              }
              profile={editor}
              harnessName={
                harnessName ?? labels[editor.harness] ?? editor.harness
              }
              apiRoot={apiRoot}
              busy={busy}
              error={error}
              onChange={setEditor}
              onSave={(p) =>
                void perform(async () => {
                  await api(
                    'upstreams',
                    Object.fromEntries(
                      Object.entries(p).filter(([k]) => k !== 'hasApiKey'),
                    ),
                  );
                  await load();
                  setEditor(null);
                })
              }
            />
          ) : (
            <form
              className="space-y-3"
              onSubmit={(e) => {
                e.preventDefault();
                void perform(async () => {
                  await api(
                    'upstreams',
                    Object.fromEntries(
                      Object.entries(editor).filter(([k]) => k !== 'hasApiKey'),
                    ),
                  );
                  await load();
                  setEditor(null);
                });
              }}
            >
              <label className="block text-sm">
                {translate('settings.name')}
                <input
                  className={field}
                  required
                  value={editor.name}
                  onChange={(e) =>
                    setEditor({ ...editor, name: e.target.value })
                  }
                />
              </label>
              <label className="block text-sm">
                {translate('settings.upstreamsHarness')}
                <select
                  className={field}
                  value={editor.harness}
                  onChange={(e) =>
                    setEditor({ ...editor, harness: e.target.value, model: '' })
                  }
                >
                  {harnessOptions.map(([id, label]) => (
                    <option key={id} value={id}>
                      {label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="block text-sm">
                {translate('settings.baseURL')}
                <input
                  className={field}
                  type="url"
                  required
                  placeholder="https://api.example.com/v1"
                  value={editor.baseUrl}
                  onChange={(e) =>
                    setEditor({ ...editor, baseUrl: e.target.value, model: '' })
                  }
                />
              </label>
              <label className="block text-sm">
                {translate('settings.aPIKey')}
                <input
                  className={field}
                  type="password"
                  autoComplete="off"
                  placeholder={
                    editor.id && editor.hasApiKey
                      ? translate('settings.leaveEmptyToKeepTheSavedKey')
                      : ''
                  }
                  required={!editor.id || !editor.hasApiKey}
                  value={editor.apiKey}
                  onChange={(e) =>
                    setEditor({ ...editor, apiKey: e.target.value, model: '' })
                  }
                />
              </label>
              <UpstreamModelPicker
                key={JSON.stringify([
                  apiRoot,
                  editor.id,
                  editor.harness,
                  editor.baseUrl,
                  editor.apiKey,
                  editor.authType,
                ])}
                apiRoot={apiRoot}
                connection={editor}
                value={editor.model}
                onChange={(model) => setEditor({ ...editor, model })}
              />
              {editor.harness === 'claude' && (
                <label className="block text-sm">
                  {translate('settings.authentication')}
                  <select
                    className={field}
                    value={editor.authType ?? 'api_key'}
                    onChange={(e) =>
                      setEditor({
                        ...editor,
                        authType: e.target.value,
                        model: '',
                      })
                    }
                  >
                    <option value="api_key">
                      {translate('settings.aPIKeyXApiKey')}
                    </option>
                    <option value="bearer">
                      {translate('settings.bearerToken')}
                    </option>
                  </select>
                </label>
              )}
              {editor.harness === 'grok' && (
                <>
                  <label className="block text-sm">
                    {translate('settings.aPIFormat')}
                    <select
                      className={field}
                      value={editor.apiType}
                      onChange={(e) =>
                        setEditor({ ...editor, apiType: e.target.value })
                      }
                    >
                      <option value="responses">Responses</option>
                      <option value="chat_completions">
                        {translate('settings.chatCompletions')}
                      </option>
                    </select>
                  </label>
                  <label className="block text-sm">
                    {translate('settings.contextWindow')}
                    <input
                      type="number"
                      min="1"
                      className={field}
                      value={editor.contextWindow}
                      onChange={(e) =>
                        setEditor({
                          ...editor,
                          contextWindow: Number(e.target.value),
                        })
                      }
                    />
                  </label>
                </>
              )}
              {error && (
                <p
                  role="alert"
                  className="text-xs text-[var(--status-danger-fg)]"
                >
                  {error}
                </p>
              )}
              <button
                className="relay-button-primary min-h-10"
                disabled={busy || !editor.model}
                type="submit"
              >
                {busy
                  ? translate('settings.saving_56a228')
                  : translate('settings.saveUpstream')}
              </button>
            </form>
          )}
        </FormDialog>
      )}
      {mode && (
        <FormDialog
          title={
            mode === 'config'
              ? translate('settings.importUpstreamConfiguration')
              : translate('settings.importDeviceTemplate')
          }
          onClose={() => setMode(null)}
          busy={busy}
        >
          <div className="space-y-3">
            {mode === 'config' && (
              <>
                <label className="block text-sm">
                  {translate('settings.upstreamsHarness')}
                  <select
                    className={field}
                    value={importHarness}
                    onChange={(e) => setImportHarness(e.target.value)}
                  >
                    {harnessOptions.map(([id, label]) => (
                      <option key={id} value={id}>
                        {label}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="block text-sm">
                  {translate('settings.name')}
                  <input
                    className={field}
                    value={importName}
                    onChange={(e) => setImportName(e.target.value)}
                  />
                </label>
                <label className="block text-sm">
                  {translate('settings.aPIKeyIfAbsentFromTheFile')}
                  <input
                    type="password"
                    autoComplete="off"
                    className={field}
                    value={importKey}
                    onChange={(e) => setImportKey(e.target.value)}
                  />
                </label>
                <p className="text-xs text-[var(--theme-fg-muted)]">
                  {translate('settings.pasteNativeTOMLJSONOrACC')}
                </p>
              </>
            )}
            <input
              ref={file}
              type="file"
              accept=".json,.toml,.txt"
              className="hidden"
              onChange={(e) => {
                const f = e.target.files?.[0];
                if (f) {
                  if (f.size > 256000) {
                    setError(
                      translate('settings.configurationMustBeSmallerThan256KB'),
                    );
                    return;
                  }
                  void f.text().then((v) => {
                    setText(v);
                    setPreview(null);
                  });
                }
              }}
            />
            <button className={button} onClick={() => file.current?.click()}>
              <Upload size={14} />
              {translate('settings.chooseFile')}
            </button>
            <textarea
              aria-label={translate('settings.configurationJSONOrTOML')}
              className={`${field} min-h-56 font-mono text-xs`}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setPreview(null);
              }}
            />
            {preview && (
              <div className="rounded-md border border-[var(--theme-border)] p-3 text-xs">
                <p>
                  {translate('settings.installIfMissing')}{' '}
                  {preview.harnesses.join(', ') || translate('settings.none')}
                </p>
                {preview.profiles.map((p) => (
                  <p className="mt-2 break-all" key={p.harness}>
                    {p.name} · {labels[p.harness]} ·{' '}
                    {p.model || translate('settings.message')}
                    <br />
                    {p.baseUrl}
                  </p>
                ))}
                <p className="mt-2">
                  {translate('settings.eachCompletedStepIsRetainedIfA')}
                </p>
              </div>
            )}
            {error && (
              <p
                role="alert"
                className="text-xs text-[var(--status-danger-fg)]"
              >
                {error}
              </p>
            )}
            <button
              className="relay-button-primary min-h-10"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  if (mode === 'config') {
                    await api('upstreams/import', {
                      harness: importHarness,
                      name: importName,
                      config: text,
                      apiKey: importKey,
                    });
                    await load();
                    setMode(null);
                    setText('');
                    setImportKey('');
                  } else {
                    let template: unknown;
                    try {
                      template = JSON.parse(text);
                    } catch (error) {
                      // The parser's own words stay as the detail after a plain explanation.
                      throw new Error(translate('settings.templateNotJson', { value1: error instanceof Error ? error.message : String(error) }));
                    }
                    if (!preview) {
                      setPreview(
                        await api('templates', { template, apply: false }),
                      );
                    } else {
                      await api('templates', { template, apply: true });
                      setJob({ state: 'running' });
                      setMode(null);
                      setText('');
                    }
                  }
                })
              }
            >
              {busy
                ? translate('settings.working')
                : mode === 'config'
                  ? translate('settings.importUpstream')
                  : preview
                    ? translate('settings.applyTemplate')
                    : translate('settings.previewTemplate')}
            </button>
          </div>
        </FormDialog>
      )}
    </section>
  );
}
