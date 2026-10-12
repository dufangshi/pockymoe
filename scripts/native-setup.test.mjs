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
  const run = (extra = [], environment = {}, input) => spawnSync('/bin/sh', [script, '--relay', 'https://relay.example.test', '--token', 'synthetic-token', '--port', '45679', ...extra], {
    encoding: 'utf8', input, env: { ...environment, HOME: root, PATH: `${bin}:/usr/bin:/bin`, TEST_ROOT: root, TEST_VERSION: version, TEST_FAIL_VERSION: failVersion ? '1' : '0', TEST_CALLS: path.join(root, 'calls'), TEST_ARGUMENTS: path.join(root, 'arguments'), TEST_BINDING: path.join(root, 'binding') },
  });
  return { root, run };
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
