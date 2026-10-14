import { appUrl } from './webBase';
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
  if (!/^[A-Za-z0-9_-]{16,512}$/.test(printableAscii(token)))
    throw new Error(translate('devices.invalidSetupToken'));
  return token;
}
const shellQuote = (value: string) =>
  `'${printableAscii(value).replace(/'/g, `'\\''`)}'`;
const powershellQuote = (value: string) =>
  `'${printableAscii(value).replace(/'/g, "''")}'`;
const powershellEnrollment = (secret: string, command: string) =>
  `$env:POCKYMOE_ENROLL_SECRET = ${powershellQuote(secret)}; try { ${command} } finally { Remove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue }`;

export function relaySupervisorCommand(
  token: string,
  platform: SupervisorPlatform,
  binding: DeviceBindingFlags,
  webOrigin = location.origin,
  origin = relayOrigin(),
) {
  validateDeviceSetupToken(token);
  if (platform === 'windows') {
    return powershellEnrollment(
      binding.enrollSecret,
      `& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing ${powershellQuote(webOrigin + appUrl('setup.ps1'))}).Content)) -Relay ${powershellQuote(origin)} -Token ${powershellQuote(token)} -Port 45680 -WebOrigin ${powershellQuote(webOrigin)} -AccountKey ${powershellQuote(binding.accountPublicKey)}`,
    );
  }
  return `curl -fsSL ${shellQuote(webOrigin + appUrl('setup.sh'))} | POCKYMOE_ENROLL_SECRET=${shellQuote(binding.enrollSecret)} sh -s -- --relay ${shellQuote(origin)} --token ${shellQuote(token)} --port 45679 --web-origin ${shellQuote(webOrigin)} --account-key ${shellQuote(binding.accountPublicKey)}`;
}

export function deviceBindCommand(
  binding: DeviceBindingFlags,
  webOrigin = location.origin,
  platform: SupervisorPlatform = 'unix',
) {
  if (platform === 'windows')
    return powershellEnrollment(
      binding.enrollSecret,
      `pockymoe bind --account-key ${powershellQuote(binding.accountPublicKey)} --web-origin ${powershellQuote(webOrigin)}`,
    );
  return `POCKYMOE_ENROLL_SECRET=${shellQuote(binding.enrollSecret)} pockymoe bind --account-key ${shellQuote(binding.accountPublicKey)} --web-origin ${shellQuote(webOrigin)}`;
}
