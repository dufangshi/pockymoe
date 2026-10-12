import { translate } from '@pockymoe/thread-ui/i18n';
import { relayOrigin } from './relayOrigin';

export type SupervisorPlatform = 'unix' | 'windows';
export interface DeviceBindingFlags {
  accountPublicKey: string;
  enrollSecret: string;
}
function printableAscii(value: string) {
  if (/[^\x20-\x7e]/.test(value))
    throw new Error(translate('devices.unsafeSetupValue'));
  return value;
}
export function validateDeviceSetupToken(token: string) {
  if (!/^[A-Za-z0-9_-]{16,512}$/.test(token))
    throw new Error(translate('devices.invalidSetupToken'));
  return token;
}
const shellQuote = (value: string) =>
  /^[A-Za-z0-9_./:@%+=,~-]+$/.test(printableAscii(value))
    ? value
    : `'${value.replace(/'/g, `'\\''`)}'`;
const powershellQuote = (value: string) => `'${printableAscii(value).replace(/'/g, "''")}'`;

export function relaySupervisorCommand(
  token: string,
  platform: SupervisorPlatform,
  binding: DeviceBindingFlags,
  webOrigin = location.origin,
  origin = relayOrigin(),
) {
  validateDeviceSetupToken(token);
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
