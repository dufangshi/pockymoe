import { describe, expect, it, vi } from 'vitest';
import {
  deviceBindCommand,
  relaySupervisorCommand,
} from './deviceSetupCommands';
vi.mock('./relayOrigin', () => ({ relayOrigin: () => 'https://relay.test' }));
const binding = {
  accountPublicKey: 'account-public-key',
  enrollSecret: 'enrollment-secret',
};
describe('bound device commands', () => {
  it('includes the account key, enrollment and separate Web origin on macOS/Linux', () => {
    const command = relaySupervisorCommand(
      'setup-token',
      'unix',
      binding,
      'https://web.test',
    );
    expect(command).toBe(
      'curl -fsSL https://web.test/setup.sh | sh -s -- --relay https://relay.test --token setup-token --port 45679 --web-origin https://web.test --account-key account-public-key --enroll enrollment-secret',
    );
  });
  it('passes PowerShell installer parameters on Windows', () => {
    const command = relaySupervisorCommand(
      'setup-token',
      'windows',
      binding,
      'https://web.test',
    );
    expect(command).toBe(
      "& ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing 'https://web.test/setup.ps1').Content)) -Relay 'https://relay.test' -Token 'setup-token' -Port 45680 -WebOrigin 'https://web.test' -AccountKey 'account-public-key' -Enroll 'enrollment-secret'",
    );
  });
  it('binds existing devices locally and shell-quotes values', () => {
    expect(deviceBindCommand(binding, 'https://web.test')).toBe(
      'pockymoe bind --account-key account-public-key --enroll enrollment-secret --web-origin https://web.test',
    );
    const windows = relaySupervisorCommand(
      "token'with quote",
      'windows',
      binding,
      'https://web.test',
    );
    expect(windows).toContain("-Token 'token''with quote'");
    const unix = relaySupervisorCommand(
      "token'with quote",
      'unix',
      binding,
      'https://web.test',
    );
    expect(unix).toContain("--token 'token'\\''with quote'");
  });
});
