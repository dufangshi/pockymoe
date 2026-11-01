import type { DesktopIndex } from '../data/types';

export interface DeviceActionContext {
  navigate(path: string): void;
  onRevealDevice?(deviceId: string): void;
  onNewThread?(deviceId: string): void;
}
export function buildDeviceActions(index: DesktopIndex, ctx: DeviceActionContext) {
  return index.devices.flatMap(device => {
    const id = encodeURIComponent(device.id);
    const action = (key: string, title: string, run: () => void) => ({ id: `device:${device.id}:${key}`, title, label: `${title} · ${device.name}`, subtitle: device.name, keywords: `${device.name} 设备 ${key}`, run });
    return [
      action('open', '在树中打开设备', () => ctx.onRevealDevice ? ctx.onRevealDevice(device.id) : ctx.navigate(`/devices/${id}/workspaces`)),
      action('stats', '查看设备统计', () => ctx.navigate(`/desktop/devices?device=${id}`)),
      action('rename', '重命名设备', () => ctx.navigate(`/desktop/devices?device=${id}&action=rename`)),
      ...(device.connected ? [
        action('new', '新建线程', () => ctx.onNewThread ? ctx.onNewThread(device.id) : ctx.navigate(`/devices/${id}/threads/new`)),
        action('update', '检查设备更新', () => ctx.navigate(`/desktop/devices?device=${id}&action=update`)),
      ] : []),
    ];
  });
}
