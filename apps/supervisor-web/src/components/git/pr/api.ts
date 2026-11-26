import { request } from '../../../lib/api';
export function sourceRoot(deviceId?: string | null) { return deviceId ? `relay/devices/${encodeURIComponent(deviceId)}/api` : 'api'; }
export function sourceAction<T = {data: unknown}>(threadId: string, action: string, input: Record<string, unknown> = {}, deviceId?: string | null) {
  return request<T>(`${sourceRoot(deviceId)}/threads/${encodeURIComponent(threadId)}/source-control/${action}`, {method:'POST',body:JSON.stringify(input)});
}
export interface ItemLink {url:string;number:number;kind:'pr'|'issue';repository:{host:string;owner:string;name:string;kind:string}}
export interface HostSettings {host:string;kind:string;enabled:boolean;hasToken:boolean;authenticated?:boolean}
export interface SourceSettings {hosts:HostSettings[];writingInstructions:string;generation?:{source?:string;model?:string;connectionId?:string};review?:import('./review/ReviewSettings').ReviewSettingsValue}
