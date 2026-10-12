import { describe, expect, it, vi } from 'vitest';
import {
  deviceBindCommand,
  relaySupervisorCommand,
  validateDeviceSetupToken,
} from './deviceSetupCommands';
vi.mock('./relayOrigin', () => ({ relayOrigin: () => 'https://relay.test' }));
const binding = {
  accountPublicKey: 'account-public-key',
  enrollSecret: 'enrollment-secret',
};
describe('bound device commands', () => {
  it.each(['unix', 'windows'] as const)(
    'keeps enrollment secrets out of %s process arguments',
    (platform) => {
      const setup = relaySupervisorCommand(
        '0123456789abcdef',
        platform,
        binding,
        'https://web.test',
      );
      const bind = deviceBindCommand(binding, 'https://web.test', platform);
      for (const command of [setup, bind]) {
        expect(command).not.toMatch(/(?:--enroll|-Enroll)\s/);
        expect(command).toContain('POCKYMOE_ENROLL_SECRET');
      }
      if (platform === 'unix') {
        expect(setup).toContain(
          "| POCKYMOE_ENROLL_SECRET='enrollment-secret' sh -s",
        );
        expect(bind).toMatch(
          /^POCKYMOE_ENROLL_SECRET='enrollment-secret' pockymoe bind/,
        );
      } else {
        expect(setup).toContain(
          "$env:POCKYMOE_ENROLL_SECRET = 'enrollment-secret'",
        );
        expect(bind).toContain(
          "$env:POCKYMOE_ENROLL_SECRET = 'enrollment-secret'",
        );
        expect(setup).toContain('finally');
        expect(bind).toContain('finally');
      }
    },
  );
  it('includes the account key, enrollment and separate Web origin on macOS/Linux', () => {
    const command = relaySupervisorCommand(
      '0123456789abcdef',
      'unix',
      binding,
      'https://web.test',
    );
    expect(command).toBe(
      "curl -fsSL 'https://web.test/setup.sh' | POCKYMOE_ENROLL_SECRET='enrollment-secret' sh -s -- --relay 'https://relay.test' --token '0123456789abcdef' --port 45679 --web-origin 'https://web.test' --account-key 'account-public-key'",
    );
  });
  it('passes PowerShell installer parameters on Windows', () => {
    const command = relaySupervisorCommand(
      '0123456789abcdef',
      'windows',
      binding,
      'https://web.test',
    );
    expect(command).toBe(
      "$env:POCKYMOE_ENROLL_SECRET = 'enrollment-secret'; try { & ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing 'https://web.test/setup.ps1').Content)) -Relay 'https://relay.test' -Token '0123456789abcdef' -Port 45680 -WebOrigin 'https://web.test' -AccountKey 'account-public-key' } finally { Remove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue }",
    );
  });
  it('binds existing devices locally and shell-quotes values', () => {
    expect(deviceBindCommand(binding, 'https://web.test')).toBe(
      "POCKYMOE_ENROLL_SECRET='enrollment-secret' pockymoe bind --account-key 'account-public-key' --web-origin 'https://web.test'",
    );
    const escaped = deviceBindCommand(
      { ...binding, accountPublicKey: "key'with quote" },
      'https://web.test',
    );
    expect(escaped).toContain("--account-key 'key'\\''with quote'");
  });
  it('clears the Windows bind secret in finally even if binding fails', () => {
    expect(deviceBindCommand(binding, 'https://web.test', 'windows')).toBe(
      "$env:POCKYMOE_ENROLL_SECRET = 'enrollment-secret'; try { pockymoe bind --account-key 'account-public-key' --web-origin 'https://web.test' } finally { Remove-Item Env:POCKYMOE_ENROLL_SECRET -ErrorAction SilentlyContinue }",
    );
  });
});

const unsafeCharacters = [
  ...Array.from({ length: 32 }, (_, index) => String.fromCharCode(index)),
  String.fromCharCode(127),
  '\u2018',
  '\u2019',
  '\u201a',
  '\u201b',
  '\u0085',
  '\u2028',
  '\u2029',
];
describe('setup command injection prevention', () => {
  it('rejects trailing controls in token validation before rendering a command', () => {
    for (const control of ['\n', '\r', '\r\n', '\u2028', '\u2029'])
      expect(() =>
        validateDeviceSetupToken('0123456789abcdef' + control),
      ).toThrow();
  });
  it.each(['unix', 'windows'] as const)(
    'rejects malformed and out-of-range %s tokens',
    (platform) => {
      for (const token of [
        '',
        'a'.repeat(15),
        'a'.repeat(513),
        "ascii'quote-token",
        'whitespace token',
        'x'.repeat(16) + '$()',
        'a'.repeat(16) + ';echo',
      ]) {
        expect(() =>
          relaySupervisorCommand(token, platform, binding, 'https://web.test'),
        ).toThrow();
      }
      for (const token of [
        'a'.repeat(16),
        'a'.repeat(512),
        'token_ABC-0123456',
      ]) {
        expect(() =>
          relaySupervisorCommand(token, platform, binding, 'https://web.test'),
        ).not.toThrow();
      }
    },
  );
  it.each(
    unsafeCharacters.map((char) => [char.codePointAt(0)!, char] as const),
  )('rejects U+%s in tokens and in every quoting path', (_codePoint, char) => {
    for (const platform of ['unix', 'windows'] as const) {
      expect(() =>
        relaySupervisorCommand(
          '0123456789abcdef' + char,
          platform,
          binding,
          'https://web.test',
        ),
      ).toThrow();
      expect(() =>
        relaySupervisorCommand(
          '0123456789abcdef',
          platform,
          { ...binding, accountPublicKey: 'key' + char },
          'https://web.test',
        ),
      ).toThrow();
      expect(() =>
        relaySupervisorCommand(
          '0123456789abcdef',
          platform,
          { ...binding, enrollSecret: 'secret' + char },
          'https://web.test',
        ),
      ).toThrow();
      expect(() =>
        relaySupervisorCommand(
          '0123456789abcdef',
          platform,
          binding,
          'https://web.test' + char,
        ),
      ).toThrow();
      expect(() =>
        relaySupervisorCommand(
          '0123456789abcdef',
          platform,
          binding,
          'https://web.test',
          'https://relay.test' + char,
        ),
      ).toThrow();
    }
    for (const platform of ['unix', 'windows'] as const) {
      expect(() =>
        deviceBindCommand(
          { ...binding, accountPublicKey: 'key' + char },
          'https://web.test',
          platform,
        ),
      ).toThrow();
      expect(() =>
        deviceBindCommand(
          { ...binding, enrollSecret: 'secret' + char },
          'https://web.test',
          platform,
        ),
      ).toThrow();
      expect(() =>
        deviceBindCommand(binding, 'https://web.test' + char, platform),
      ).toThrow();
    }
  });
});
