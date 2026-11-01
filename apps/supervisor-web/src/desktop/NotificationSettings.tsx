import { Bell, Sparkles } from 'lucide-react';
import { ErrorMark } from '@pockymoe/thread-ui';
export default function DesktopNotificationSettings() {
  const host = window.pockymoeHost;
  const supported = typeof host?.window?.openPreferences === 'function';
  return <section className="space-y-2 py-4" aria-label="系统通知与桌宠"><h3 className="text-sm font-medium">系统通知 · 桌宠</h3><p className="text-xs text-[var(--theme-fg-muted)]">提醒方式和桌宠显示由桌面应用管理。</p><div className="flex items-center gap-2"><button className="relay-button-secondary inline-flex items-center gap-2" disabled={!supported} onClick={() => host?.window.openPreferences('general')}><Bell size={14} data-icon="notifications" />系统通知 · 桌宠<Sparkles size={14} /></button>{!supported ? <ErrorMark kind="unavailable" message="请更新桌面外壳以打开通知偏好设置。" /> : null}</div></section>;
}
