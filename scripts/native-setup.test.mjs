import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./setup.sh', import.meta.url));
function fixture(t, { version = '9.1.0', corrupt = false, failVersion = false, platform = 'Linux', arch = 'x86_64' } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'native-bootstrap-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const bin = path.join(root, 'bin'); fs.mkdirSync(bin);
  const asset = platform === 'Darwin' ? 'remote-codex-darwin-arm64' : `remote-codex-linux-${arch === 'aarch64' ? 'arm64' : 'x64'}-gnu`;
  const native = '#!/bin/sh\nif [ "$1" = version ]; then printf "%s\\n" "$TEST_VERSION"; else printf "%s\\n" "$@" > "$TEST_ARGUMENTS"; printf "%s\\n%s\\n%s\\n" "$POCKYMOE_WEB_ORIGIN" "$POCKYMOE_ACCOUNT_KEY" "$POCKYMOE_ENROLL_SECRET" > "$TEST_BINDING"; fi\n';
  const sums = `${corrupt ? 'a'.repeat(64) : crypto.createHash('sha256').update(native).digest('hex')}  ${asset}\n`;
  fs.writeFileSync(path.join(root, 'binary'), native);
  fs.writeFileSync(path.join(root, 'sums'), sums);
  fs.writeFileSync(path.join(root, 'version'), version + '\n');
  fs.writeFileSync(path.join(bin, 'uname'), `#!/bin/sh\ncase "$1" in -s) echo ${platform};; -m) echo ${arch};; esac\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, 'curl'), `#!${process.execPath}
const fs=require('fs'),path=require('path');const args=process.argv.slice(2);fs.appendFileSync(process.env.TEST_CALLS,JSON.stringify(args)+'\\n');
const url=args.find(a=>a.startsWith('https://'));let file;
if(url.endsWith('/runtime-version.txt')){if(process.env.TEST_FAIL_VERSION==='1')process.exit(22);file='version';}
else if(url.endsWith('/SHA256SUMS'))file='sums';else if(url.endsWith('/${asset}'))file='binary';else process.exit(22);
fs.copyFileSync(path.join(process.env.TEST_ROOT,file),args[args.indexOf('-o')+1]);
`, { mode: 0o755 });
  for (const name of ['node', 'npm']) fs.writeFileSync(path.join(bin, name), '#!/bin/sh\necho "Unexpected Node/npm invocation" >&2\nexit 99\n', { mode: 0o755 });
  const runWith = (args, environment = {}, input, shell = '/bin/sh') => spawnSync(shell, [script, ...args], {
    encoding: 'utf8', input, env: { ...environment, HOME: root, PATH: `${bin}:/usr/bin:/bin`, TEST_ROOT: root, TEST_VERSION: version, TEST_FAIL_VERSION: failVersion ? '1' : '0', TEST_CALLS: path.join(root, 'calls'), TEST_ARGUMENTS: path.join(root, 'arguments'), TEST_BINDING: path.join(root, 'binding') },
  });
  const run = (extra = [], environment = {}, input, shell = '/bin/sh') => runWith(['--relay', 'https://relay.example.test', '--token', 'synthetic-token', '--port', '45679', ...extra], environment, input, shell);
  return { root, run, runWith };
}
test('native SH bootstrap installs verified GitHub executable without Node/npm on Linux and Apple Silicon', t => {
  for (const platform of [{}, { arch: 'aarch64' }, { platform: 'Darwin', arch: 'arm64' }]) {
    const f = fixture(t, platform); const result = f.run();
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(fs.readFileSync(path.join(f.root, 'arguments'), 'utf8').trim().split('\n'), ['setup', '--relay', 'https://relay.example.test', '--token', 'synthetic-token', '--port', '45679']);
    const calls = fs.readFileSync(path.join(f.root, 'calls'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(calls.length, 3);
    assert(calls.every(c => c.includes('--progress-bar') && c.includes('--speed-time')));
    assert(calls.some(c => c.some(a => a.includes('/releases/download/v9.1.0/'))));
    assert(!result.stdout.includes('synthetic-token'));
    assert.deepEqual(fs.readdirSync(path.join(f.root, '.local/share/remote-codex')), []);
  }
});
test('failed metadata, invalid versions and corrupt assets stop before native setup with an actionable error', t => {
  for (const [options, error] of [[{ failVersion: true }, /Could not resolve/], [{ version: '../escape' }, /invalid runtime version/], [{ corrupt: true }, /checksum verification failed/]]) {
    const f = fixture(t, options); const result = f.run();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert(!fs.existsSync(path.join(f.root, 'arguments')));
    assert.deepEqual(fs.readdirSync(path.join(f.root, '.local/share/remote-codex')), []);
  }
});

test('native SH bootstrap forwards both binding flags privately and rejects incomplete binding before download', t => {
  const f = fixture(t);
  const result = f.run(['--web-origin', 'https://web.test', '--account-key', 'synthetic-public'], {POCKYMOE_ENROLL_SECRET:'synthetic-enrollment'});
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(fs.readFileSync(path.join(f.root, 'binding'), 'utf8').trim().split('\n'), ['https://web.test', 'synthetic-public', 'synthetic-enrollment']);
  assert(!result.stdout.includes('synthetic-enrollment'));
  assert(!result.stderr.includes('synthetic-enrollment'));
  for (const args of [['--account-key', 'synthetic-public'], ['--enroll', 'synthetic-enrollment']]) {
    const incomplete = fixture(t); const refused = incomplete.run(args);
    assert.notEqual(refused.status, 0);
    assert(!fs.existsSync(path.join(incomplete.root, 'calls')));
  }
});

test('enrollment argv is refused and stdin reaches native setup without an argv secret', t => {
  const rejected = fixture(t); const result = rejected.run(['--account-key','synthetic-public','--enroll','synthetic-enrollment']);
  assert.notEqual(result.status,0); assert(!fs.existsSync(path.join(rejected.root,'arguments')));
  const stdin = fixture(t); const accepted = stdin.run(['--account-key','synthetic-public','--enroll-stdin'],{},'synthetic-enrollment\n');
  assert.equal(accepted.status,0,accepted.stderr);
  assert.equal(fs.readFileSync(path.join(stdin.root,'binding'),'utf8').trim().split('\n').at(-1),'synthetic-enrollment');
  assert(!fs.readFileSync(path.join(stdin.root,'arguments'),'utf8').includes('synthetic-enrollment'));
});
test('Windows bootstrap does not put enrollment secret into native command arguments', () => {
  const source = fs.readFileSync(new URL('./setup.ps1',import.meta.url),'utf8');
  assert(!source.includes("'--enroll'"));
  assert(!source.includes('[string]$Enroll,'));
  assert(source.includes('POCKYMOE_ENROLL_SECRET'));
});

test('bootstrap scripts are ASCII so macOS /bin/sh (bash 3.2) cannot misread variable names', () => {
  // bash 3.2 on macOS treats a UTF-8 byte right after "$name" as part of the
  // name ("setup_version?: unbound variable" under set -u).
  for (const name of ['./setup.sh', './setup.ps1']) {
    const source = fs.readFileSync(new URL(name, import.meta.url));
    const offset = source.findIndex(byte => byte > 0x7e || (byte < 0x20 && byte !== 0x0a && byte !== 0x0d && byte !== 0x09));
    assert.equal(offset, -1, `${name} has a non-ASCII byte at offset ${offset}`);
  }
});
test('native SH bootstrap runs under bash and forwards --reset-database to native setup', t => {
  for (const shell of ['/bin/sh', 'bash']) {
    const f = fixture(t); const result = f.run(['--reset-database'], {}, undefined, shell);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(fs.readFileSync(path.join(f.root, 'arguments'), 'utf8').trim().split('\n'), ['setup', '--relay', 'https://relay.example.test', '--token', 'synthetic-token', '--port', '45679', '--reset-database']);
    assert.match(result.stdout, /Configuring Pockymoe 9\.1\.0\.\.\./);
  }
});
test('Windows bootstrap forwards -ResetDatabase', () => {
  const source = fs.readFileSync(new URL('./setup.ps1', import.meta.url), 'utf8');
  assert(source.includes('[switch]$ResetDatabase'));
  assert(source.includes("if ($ResetDatabase) { $arguments += '--reset-database' }"));
});

test('without relay credentials the bootstrap installs local mode with only the options given', t => {
  for (const [args, expected] of [
    [[], ['setup', '--local']],
    [['--local'], ['setup', '--local']],
    [['--port', '9000', '--lan'], ['setup', '--local', '--port', '9000', '--lan']],
    [['--no-lan', '--reset-database'], ['setup', '--local', '--reset-database', '--no-lan']],
  ]) {
    const f = fixture(t); const result = f.runWith(args, { POCKYMOE_WEB_ORIGIN: 'https://ignored.test' });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(fs.readFileSync(path.join(f.root, 'arguments'), 'utf8').trim().split('\n'), expected);
    // A local install is never bound to a relay account.
    assert.deepEqual(fs.readFileSync(path.join(f.root, 'binding'), 'utf8').split('\n').slice(0, 3), ['', '', '']);
  }
});
test('local and relay options cannot be mixed', t => {
  for (const args of [['--local', '--token', 'synthetic-token'], ['--account-key', 'synthetic-public'], ['--relay', 'https://relay.example.test', '--token', 't', '--lan']]) {
    const f = fixture(t); const result = f.runWith(args);
    assert.notEqual(result.status, 0, args.join(' '));
    assert(!fs.existsSync(path.join(f.root, 'calls')), args.join(' '));
  }
});
