import { useEffect, useRef, useState } from 'react';
import { Folder, Monitor } from 'lucide-react';
import { useDesktopSelector } from '../data';
export function LocationPicker({ onClose, onChoose }: { onClose(): void; onChoose(deviceId: string, workspaceId: string): void }) {
  const index = useDesktopSelector(index => index);
  const [deviceId, setDeviceId] = useState(index.devices.find(device => device.connected)?.id ?? index.devices[0]?.id ?? '');
  const [workspaceId, setWorkspaceId] = useState('');
  const form = useRef<HTMLFormElement>(null);
  const workspaces = index.workspaces.filter(workspace => workspace.deviceId === deviceId);
  useEffect(() => { form.current?.querySelector('select')?.focus(); }, []);
  return <div className="rc-modal-backdrop" onMouseDown={event => { if (event.currentTarget === event.target) onClose(); }}><form ref={form} className="rc-rename-dialog rc-location-dialog" role="dialog" aria-modal="true" aria-label="选择线程位置" onKeyDown={event => { if (event.key === 'Escape') onClose(); }} onSubmit={event => { event.preventDefault(); if (deviceId) onChoose(deviceId, workspaceId || workspaces[0]?.id || ''); }}>
    <h2>选择线程位置</h2><p>在哪台设备上开始工作？</p>
    <label><Monitor size={14} />设备<select aria-label="新线程设备" value={deviceId} onChange={event => { setDeviceId(event.target.value); setWorkspaceId(''); }}>{index.devices.map(device => <option key={device.id} value={device.id}>{device.name}{device.connected ? '' : ' · 离线'}</option>)}</select></label>
    <label><Folder size={14} />工作区<select aria-label="新线程工作区" value={workspaceId || workspaces[0]?.id || ''} onChange={event => setWorkspaceId(event.target.value)}>{workspaces.length ? workspaces.map(workspace => <option key={workspace.key} value={workspace.id}>{workspace.label}</option>) : <option value="">在下一步选择工作区</option>}</select></label>
    <div><button type="button" onClick={onClose}>取消</button><button type="submit" disabled={!deviceId || !index.devices.find(device => device.id === deviceId)?.connected}>继续</button></div>
  </form></div>;
}
