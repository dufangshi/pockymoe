import { useEffect, useState } from 'react';
import { confirmAsync, CopyButton, ErrorMark } from '@pockymoe/thread-ui';
import { getLocale, useI18n } from '@pockymoe/thread-ui/i18n';
import { request } from '../lib/api';
import { FormDialog } from './FormDialog';
import { setupCommands, type SetupCommand } from '../lib/outsideAgentSetup';

const words = {
  title: ['外部智能体', 'Outside agents'], allow: ['允许', 'Allow'], deny: ['拒绝', 'Deny'],
  read: ['只读', 'Read only'], supervised: ['需审批', 'Supervised'], full: ['完整访问', 'Full access'],
  workspaces: ['工作区', 'Workspaces'], all: ['所有工作区', 'All workspaces'],
  clients: ['已连接的客户端', 'Connected clients'], calls: ['最近调用', 'Recent calls'],
  revoke: ['撤销', 'Revoke'], refresh: ['刷新', 'Refresh'],
  lastUse: ['最后使用', 'Last used'], setup: ['连接设置', 'Setup'],
  note: ['此设备保存授权。需审批访问只能操作受保护线程。', 'Grants are saved on this device. Supervised access operates guarded threads.'],
  redirect: ['返回地址', 'Redirect URI'], access: ['访问级别', 'Access level'],
  copy: ['复制设置', 'Copy setup'],
  local: ['本机（stdio）', 'This computer (stdio)'], http: ['HTTP（OAuth 登录）', 'HTTP (OAuth sign-in)'],
  flags: ['默认需审批。可加 --access read|full、--workspace 工作区ID。', 'Supervised by default. Add --access read|full or --workspace WORKSPACE_ID.'],
  empty: ['暂无', 'None'], loading: ['加载中…', 'Loading…'],
  revokeTitle: ['撤销 {name}？', 'Revoke {name}?'],
  revokeBody: ['它的令牌立即失效；要再次使用，需要重新登录并获得你的同意。', 'Its tokens stop working now; to use Pockymoe again it must sign in and get your consent.'],
  stdioClient: ['本机 stdio', 'Local stdio'], revokedClient: ['已撤销的客户端', 'Revoked client'],
  signIn: ['请先在此设备登录，然后重试。', 'Sign in to this device, then retry.'],
} as const;
const text = (key: keyof typeof words) => words[key][getLocale() === 'zh-CN' ? 0 : 1];
type Access = 'read' | 'supervised' | 'full';
type Client = { id: string; clientName: string; access: Access; workspaces: string[] | null; lastUsedAt: number };
type Call = { client: string; clientName?: string; tool: string; target: string | null; result: string; time: number };
type Data = { endpoint: string; clients: Client[]; calls: Call[] };
const post = (body: unknown) => ({ method: 'POST', body: JSON.stringify(body) });

export function OutsideAgentsSettings() {
  useI18n();
  const [data, setData] = useState<Data | null>(null);
  const [error, setError] = useState<string | null>(null);
  async function load() { try { setData(await request<Data>('api/mcp/clients')); setError(null); } catch (e) { setError(String(e)); } }
  useEffect(() => { void load(); }, []);
  const [workspaceNames, setWorkspaceNames] = useState<Record<string, string>>({});
  useEffect(() => { request<{ id: string; label: string }[]>('api/workspaces').then(list => setWorkspaceNames(Object.fromEntries(list.map(w => [w.id, w.label])))).catch(() => {}); }, []);
  async function revoke(client: Client) {
    const confirmed = await confirmAsync({ title: text('revokeTitle').replace('{name}', client.clientName), description: text('revokeBody'), confirmLabel: text('revoke') });
    if (!confirmed) return;
    try { await request(`api/mcp/clients/${encodeURIComponent(client.id)}`, { method: 'DELETE' }); await load(); } catch (e) { setError(String(e)); }
  }
  const time = (seconds: number) => new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: 'short', timeStyle: 'short' });
  const callClient = (call: Call) => call.clientName ?? (call.client === 'local-stdio' ? text('stdioClient') : text('revokedClient'));
  const setup = setupCommands(data?.endpoint);
  const rows = (list: SetupCommand[]) => list.map(([app, command]) => <div key={app}><b>{app}</b><div className="flex items-start gap-2"><code className="min-w-0 whitespace-pre-wrap break-all text-xs">{command}</code><CopyButton value={command} label={text('copy')} /></div></div>);
  return <section className="space-y-4 min-w-0">
    <div className="flex items-center justify-between gap-3"><p className="settings-note min-w-0 text-sm">{text('note')}</p><span className="flex shrink-0 items-center gap-2">{error && <ErrorMark message={error} />}<button className="relay-button-secondary px-3 py-2" onClick={() => void load()}>{text('refresh')}</button></span></div>
    <details><summary>{text('setup')}</summary><div className="space-y-3 py-3">
      <h4 className="text-xs font-semibold">{text('local')}</h4>{rows(setup.local)}<p className="settings-note text-xs">{text('flags')}</p>
      {data && <><h4 className="text-xs font-semibold">{text('http')}</h4><div className="flex gap-2"><code className="min-w-0 break-all text-xs">{data.endpoint}</code><CopyButton value={data.endpoint} label={text('copy')} /></div>{rows(setup.http)}</>}
    </div></details>
    <h3 className="text-[13px] font-semibold">{text('clients')}</h3>
    {data?.clients.length === 0 && <p>{text('empty')}</p>}
    {data?.clients.map(client => <div key={client.id} className="flex items-center gap-3 rounded border border-[var(--theme-border)] px-3 py-2">
      <div className="min-w-0 flex-1"><b className="block truncate" title={client.clientName}>{client.clientName}</b>
        <p className="settings-note text-xs">{text(client.access)} · {client.workspaces?.map(id => workspaceNames[id] ?? id).join(', ') ?? text('all')} · {text('lastUse')} {time(client.lastUsedAt)}</p></div>
      <button className="relay-button-secondary shrink-0 px-3 py-2 text-[var(--status-danger-fg)]" onClick={() => void revoke(client)}>{text('revoke')}</button>
    </div>)}
    <details><summary>{text('calls')}</summary><div>{data?.calls.map((call, index) => <div key={index} className="border-b border-[var(--theme-border)] py-2 text-xs"><p className="break-all"><b>{call.tool.replace(/^pockymoe_/, '')}</b> · {call.result} · {callClient(call)}</p><p className="settings-note">{time(call.time)}{call.target && <> · <span className="break-all">{call.target}</span></>}</p></div>)}</div></details>
  </section>;
}

type Consent = { client: { client_name: string }; query: { redirect_uri: string }; requestedAccess: Access };
export function OutsideAgentConsent() {
  useI18n();
  const [ticket, setTicket] = useState(() => new URLSearchParams(location.search).get('mcpConsent'));
  const [consent, setConsent] = useState<Consent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [access, setAccess] = useState<Access>('read');
  const [workspaces, setWorkspaces] = useState<{ id: string; label: string }[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [all, setAll] = useState(true);
  const [busy, setBusy] = useState(false);
  async function load() { try { const c = await request<Consent>(`api/mcp/consent/${encodeURIComponent(ticket!)}`); setConsent(c); setAccess(c.requestedAccess); const ws = await request<{ id: string; label: string }[]>('api/workspaces'); setWorkspaces(ws); setError(null); } catch { setError(text('signIn')); } }
  useEffect(() => { if (ticket) void load(); }, [ticket]); // eslint-disable-line react-hooks/exhaustive-deps
  function close() { setTicket(null); const u = new URL(location.href); u.searchParams.delete('mcpConsent'); history.replaceState(null, '', u); }
  async function answer(approve: boolean) {
    if (busy) return;
    setBusy(true);
    try { const result = await request<{ redirect: string }>(`api/mcp/consent/${encodeURIComponent(ticket!)}`, post({ approve, access, workspaces: all ? null : selected })); location.assign(result.redirect); }
    catch (e) { setError(String(e)); setBusy(false); }
  }
  if (!ticket) return null;
  return <FormDialog title={text('title')} onClose={() => { if (consent) void answer(false); else close(); }} focusCancel>
    {consent ? <div className="space-y-4">
      <b className="break-all">{consent.client.client_name}</b><p className="break-all text-sm">{text('redirect')}: {consent.query.redirect_uri}</p>
      <fieldset><legend>{text('access')}</legend><div className="product-segmented">{(['read', 'supervised', 'full'] as Access[]).slice(0, ['read', 'supervised', 'full'].indexOf(consent.requestedAccess) + 1).map(level => <button className="product-segment" key={level} aria-pressed={access === level} onClick={() => setAccess(level)}>{text(level)}</button>)}</div></fieldset>
      <fieldset className="space-y-2"><legend>{text('workspaces')}</legend><label className="flex gap-2"><input type="checkbox" checked={all} onChange={e => setAll(e.target.checked)} />{text('all')}</label>{!all && workspaces.map(w => <label key={w.id} className="flex gap-2"><input type="checkbox" checked={selected.includes(w.id)} onChange={e => setSelected(e.target.checked ? [...selected, w.id] : selected.filter(id => id !== w.id))} />{w.label}</label>)}</fieldset>
      <p className="settings-note text-sm">{text('note')}</p>
    </div> : <p>{text('loading')}</p>}
    {error && <div className="flex gap-2"><ErrorMark message={error} /><button onClick={() => void load()}>{text('refresh')}</button></div>}
    <div className="flex justify-end gap-2 mt-4"><button data-dialog-cancel className="relay-button-secondary px-3 py-2" onClick={() => { if (consent) void answer(false); else close(); }}>{text('deny')}</button><button className="relay-button-primary px-3 py-2" disabled={!consent || busy} onClick={() => void answer(true)}>{text('allow')}</button></div>
  </FormDialog>;
}
