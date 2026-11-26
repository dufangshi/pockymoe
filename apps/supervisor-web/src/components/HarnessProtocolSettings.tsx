import { useEffect, useState } from 'react';
import { ErrorMark, Select } from '@pockymoe/thread-ui';
import { getLocale, useI18n } from '@pockymoe/thread-ui/i18n';
import { deviceRequest } from '../lib/api';
type Protocol = 'acp' | 'native';
type Settings = Record<'codex' | 'claude', Protocol>;
export function HarnessProtocolSettings({ deviceId }: { deviceId: string | null }) {
  useI18n();
  const zh = getLocale() === 'zh-CN';
  const [settings, setSettings] = useState<Settings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    let alive = true;
    deviceRequest<Settings>(deviceId, 'api/harness-protocols/settings', { cache: 'no-store' })
      .then(value => { if (alive) { setSettings(value); setError(null); } })
      .catch((cause: unknown) => { if (alive) setError(String(cause)); });
    return () => { alive = false; };
  }, [deviceId, attempt]);
  async function change(agent: keyof Settings, protocol: Protocol) {
    if (!settings || busy) return;
    setBusy(true);
    try {
      const value = await deviceRequest<Settings>(deviceId, 'api/harness-protocols/settings', {
        method: 'PUT', body: JSON.stringify({ ...settings, [agent]: protocol }),
      });
      setSettings(value); setError(null);
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(false); }
  }
  return <section className="pm-res-section" aria-label={zh ? '智能体协议' : 'Harness protocols'}>
    <h3 className="pm-res-heading">{zh ? '智能体协议' : 'Harness protocols'}</h3>
    <p className="text-xs text-[var(--theme-fg-muted)]">{zh ? '下次加载会话时生效。原生协议为实验功能；ACP 为默认。' : 'Applies when a session next loads. Native is experimental; ACP is the default.'}</p>
    {(['codex', 'claude'] as const).map(agent => <label key={agent} className="flex items-center justify-between gap-3 py-2 text-sm">
      <span>{agent === 'codex' ? 'Codex' : 'Claude Code'}</span>
      <Select aria-label={`${agent === 'codex' ? 'Codex' : 'Claude Code'} ${zh ? '协议' : 'protocol'}`} className="host-input rounded-md border p-2" value={settings?.[agent] ?? 'acp'} disabled={!settings || busy} onValueChange={value => void change(agent, value as Protocol)}>
        <option value="acp">ACP</option><option value="native">{zh ? '原生' : 'Native'}</option>
      </Select>
    </label>)}
    {error && <ErrorMark message={error} onRetry={() => setAttempt(value => value + 1)} retryLabel={zh ? '重试' : 'Retry'} />}
  </section>;
}
