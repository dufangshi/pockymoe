import { relayOrigin } from './relayOrigin';

export type SupervisorPlatform = 'unix' | 'windows';
export interface DeviceBindingFlags {
  accountPublicKey: string;
  enrollSecret: string;
}
const shellQuote = (value: string) =>
  /^[A-Za-z0-9_./:@%+=,~-]+$/.test(value)
    ? value
    : `'${value.replace(/'/g, `'\\''`)}'`;
const powershellQuote = (value: string) => `'${value.replace(/'/g, "''")}'`;

export function relaySupervisorCommand(
  token: string,
  platform: SupervisorPlatform,
  binding: DeviceBindingFlags,
  webOrigin = location.origin,
  origin = relayOrigin(),
) {
  if (platform === 'windows') {
    return `& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing ${powershellQuote(webOrigin + '/setup.ps1')}).Content)) -Relay ${powershellQuote(origin)} -Token ${powershellQuote(token)} -Port 45680 -WebOrigin ${powershellQuote(webOrigin)} -AccountKey ${powershellQuote(binding.accountPublicKey)} -Enroll ${powershellQuote(binding.enrollSecret)}`;
  }
  return `curl -fsSL ${shellQuote(webOrigin + '/setup.sh')} | sh -s -- --relay ${shellQuote(origin)} --token ${shellQuote(token)} --port 45679 --web-origin ${shellQuote(webOrigin)} --account-key ${shellQuote(binding.accountPublicKey)} --enroll ${shellQuote(binding.enrollSecret)}`;
}

export function deviceBindCommand(
  binding: DeviceBindingFlags,
  webOrigin = location.origin,
) {
  return `pockymoe bind --account-key ${shellQuote(binding.accountPublicKey)} --enroll ${shellQuote(binding.enrollSecret)} --web-origin ${shellQuote(webOrigin)}`;
}
