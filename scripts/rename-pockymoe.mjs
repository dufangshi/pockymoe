#!/usr/bin/env node
// Re-runnable rename codemod: Remote Codex -> Pockymoe.
//
// Usage: node scripts/rename-pockymoe.mjs [repoRoot] [--dry-run]
//
// The rename branch is produced by running this script on a fresh main and
// committing the result, so it can be rebased by re-running it instead of
// resolving conflicts by hand. Other branches can run it on their own tree
// before merging main after the rename lands.
//
// Persisted or wire-level identifiers that older installations, browsers,
// relays or native apps still read are listed in KEEP and left unchanged; the
// runtime accepts the old names through explicit compatibility code.

import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const args = process.argv.slice(2);
const dryRun = args.includes('--dry-run');
const root = path.resolve(args.find((arg) => !arg.startsWith('--')) ?? '.');

// Paths left untouched: lockfiles are regenerated, historical records keep the
// name that was current when they were written, and the naming proposal is the
// record of the rename itself.
const SKIP_PATHS = [
  /(^|\/)pnpm-lock\.yaml$/,
  /(^|\/)Cargo\.lock$/,
  // Generated bundles are rebuilt from the renamed sources.
  /(^|\/)dist\//,
  /^docs\/proposals\/naming\//,
  /^docs\/release-[^/]+\.md$/,
  /^docs\/incident-[^/]+\.md$/,
  /^docs\/operations-[^/]+\.md$/,
  /^scripts\/rename-pockymoe\.mjs$/,
  // Independently released bootstrap: only its visible strings change, by hand,
  // in a Device Manager release.
  /^apps\/windows-device-manager\//,
  // Hosted infrastructure: installed paths, units, users, Incus projects and
  // env files on running hosts and guests change in a coordinated infra rollout.
  /^packages\/incus-host-agent\/(?!package\.json$|README\.md$)/,
  /^\.github\/workflows\/(relay-deploy|hosted-runtime-rollout|incus-host-agent-deploy)\.yml$/,
  /^scripts\/rollout-runtime\.sh$/,
  // The retired npm launcher and the tests and live drivers of its shipped
  // behaviour: legacy devices still run published copies of it, by its old
  // names, until native setup migrates them.
  /^npm\//,
  /^scripts\/(installation|npm-launcher-download|npm-launcher-relay|supervisor-update|publish-npm-release|setup)\.test\.mjs$/,
  /^scripts\/(pack-npm-release|publish-npm-release|verify-npm-package|device-setup-live|test-supervisor-update-live|test-supervisor-restart-live|test-supervisor-relay-restart-live)\.mjs$/,
];

// Exact substrings that must survive the rename. Each entry is either external
// (owned by another system until it is renamed there) or persisted/wire state
// shared with older devices, browsers, relays, native apps or harness configs.
const KEEP = [
  // External: repositories, domains and the separate native app repository.
  'github.com/dufangshi/remoteCodex',
  'dufangshi/remoteCodex',
  'dufangshi/remote-codex-thread-ui-rust',
  'dufangshi/remote-codex-thread-ui',
  'remote-codex-thread-ui-rust',
  'remote-codex-app',
  'relay.example.com',
  'legacy-relay.example.com',
  // Local checkout paths used in docs and scripts.
  '/home/ubuntu/dev/remoteCodex',
  'remoteCodex.worktrees',
  // End-to-end encryption, stored-secret derivation and legacy password hashes.
  'remote-codex/relay/v1',
  'remote-codex/http-response/v1',
  'remote-codex/ws-client/v1',
  'remote-codex/ws-server/v1',
  'remote-codex/totp-storage/v1',
  'remote-codex/device-setup-storage/v1',
  '__remote_codex_legacy_sha256__',
  // Applied-migrations table.
  '__remote_codex_runtime_migrations',
  // Session, MFA and OAuth cookies, and the preview proxy's cookie filter.
  'remote_codex_relay_session',
  'remote_codex_relay_admin_session',
  'remote_codex_trusted_browser',
  'remote_codex_factor_challenge',
  'remote_codex_oauth',
  'remote_codex_session',
  'remote_codex_relay_',
  'X-Remote-Codex-Auth-Realm',
  'x-remote-codex-auth-realm',
  // Browser storage, trust-on-first-use pins and cross-bundle event names.
  'remote-codex-auth-token',
  'remote-codex-relay-token',
  'remote-codex-relay-admin-token',
  'remote-codex-relay-mode',
  'remote-codex-relay-device-id',
  'remote-codex-relay-thread-id',
  'remote-codex-relay-return-to',
  'remote-codex-theme-mode',
  'remote-codex-default-backend',
  'remote-codex-auto-collapse-completed-turns',
  'remote-codex-show-reasoning-summaries',
  'remote-codex-font-size',
  'remote-codex.local-workbench.v1',
  'remote-codex:shell-layout:',
  'remote-codex:thread-seen:',
  'remote-codex.presentation.v1',
  'remote-codex.locale',
  'remote-codex.explorer-width',
  'remote-codex:graphchat:',
  'remote-codex-transport',
  'remote-codex-locale',
  'remote-codex-reset-transport',
  'remote-codex-current-route',
  'remote-codex-account-updated',
  'remote-codex:hosted-vm-wake',
  'remote-codex.i18n',
  'remote-codex.workspace-documents',
  'remote-codex-workspace',
  // Native app bridges.
  'remoteCodexNative',
  'messageHandlers.remoteCodex',
  'messageHandlers?.remoteCodex',
  // Persisted plugin IDs, transcript artifacts and the molecule MCP tool.
  'remote-codex.terminal',
  'remote-codex.deepseek-harness',
  'remote-codex.workspace-molecule-preview',
  'remote-codex-artifact',
  'remote-codex.artifact',
  'remote_codex_render_molecule',
  // Entries written into users' Codex, Grok and DSH configuration.
  '"remote_codex"',
  '{remote_codex',
  '["remote_codex"]',
  '["remote_codex", "remote-codex"]',
  'remote-codex-bridge',
  'providers.remote-codex',
  'REMOTE_CODEX_DSH_API_KEY',
  '"REMOTE_CODEX_DSH_{}"',
  // Grok model aliases written as `remote-codex/<model>`.
  '"remote-codex/',
  'remote-codex/{',
  'remote-codex/grok',
  'remote-codex/model-',
  'remote-codex/obsolete',
  // Plugin manifests declare a `remoteCodex` host version range; native app
  // bridges register a `remoteCodex` message handler.
  '"remoteCodex"',
  'remoteCodex:',
  'remoteCodex?:',
  'value.remoteCodex',
  "'remoteCodex'",
  'pub remote_codex: String',
  // Data directories shared with the independently released Windows Device
  // Manager, installed updaters and rollback (relay-supervisor.json, the
  // transport identity, SQLite stores, the native binary cache).
  '.remote-codex',
  'share/remote-codex',
  // Shipped Device Managers parse these keys from the portal's Windows snippet.
  '$env:REMOTE_CODEX_RELAY_',
  // GitHub release assets that installed native updaters download by name.
  'remote-codex-linux-x64-gnu',
  'remote-codex-linux-arm64-gnu',
  'remote-codex-darwin-arm64',
  'remote-codex-win32-x64-msvc-cli.exe',
  'remote-codex-web.zip',
  'remote-codex-${setup_os}',
  'remote-codex-linux-${',
  // The npm launcher entry that native setup bridges on legacy devices.
  'remote-codex.mjs',
  'npm/remote-codex',
  // Installed services, sessions and hosted infrastructure on running machines.
  'remote-codex-supervisor.service',
  'com.remote-codex.supervisor',
  'com.remotecodex.update',
  'remote-codex-relay-supervisor',
  'remote-codex-rust-relay',
  // Data volume of Dockerfile.relay images already deployed by self-hosters.
  '/var/lib/remote-codex-relay',
  'remote-codex-relay-net',
  'remote-codex-hosted',
  '/home/remote-codex',
  '/etc/remote-codex',
  'REMOTE_CODEX_RELAY_DEPLOY_',
];

// Per-file literals: the bare provider and model IDs written into users' Grok
// and DSH configuration are only meaningful in these modules.
const FILE_KEEP = [
  [/^crates\/runtime\/src\/upstreams\.rs$/, ['"remote-codex"']],
  [/^crates\/runtime\/src\/upstreams\/dsh\.rs$/, ['"remote-codex"']],
  [/^crates\/runtime\/src\/acp\/grok\.rs$/, ['"remote-codex"']],
  // Native installs run `native/current/remote-codex` from service units and
  // link it as ~/.local/bin/remote-codex; Pockymoe names are added beside them.
  [/^crates\/supervisor\/src\/distribution\/(mod|releases)\.rs$/, ['"remote-codex"', '"remote-codex.exe"']],
  // Native setup bridges legacy npm installs of the `remote-codex` package.
  [/^crates\/supervisor\/src\/distribution\/setup\.rs$/, ['"remote-codex"']],
];

const REPLACEMENTS = [
  // Context markers injected into agent prompts read as prose.
  [/\[remoteCodex\b/g, '[Pockymoe'],
  [/\bremoteCodex(?= )/g, 'Pockymoe'],
  ['@dufangshi/remote-codex-native-', '@dufangshi/pockymoe-native-'],
  ['remote-codex-thread-ui', 'pockymoe-thread-ui'],
  ['@remote-codex/', '@pockymoe/'],
  ['Remote Codex', 'Pockymoe'],
  ['Remote-Codex', 'Pockymoe'],
  ['Remote codex', 'Pockymoe'],
  ['remote codex', 'Pockymoe'],
  ['REMOTE CODEX', 'POCKYMOE'],
  ['RemoteCodex', 'Pockymoe'],
  ['remoteCodex', 'pockymoe'],
  ['remotecodex', 'pockymoe'],
  ['REMOTE_CODEX', 'POCKYMOE'],
  ['remote_codex', 'pockymoe'],
  ['remote-codex', 'pockymoe'],
];

const PATH_REPLACEMENTS = [
  ['remote-codex', 'pockymoe'],
  ['remote_codex', 'pockymoe'],
  ['RemoteCodex', 'Pockymoe'],
  ['remoteCodex', 'pockymoe'],
];

export function renameText(text, keep = KEEP) {
  // Pattern entries are regular expressions; the rest are literal substrings.
  const ordered = [...keep].sort((a, b) => b.length - a.length);
  const tokens = [];
  let out = text;
  for (const literal of ordered) {
    if (!out.includes(literal)) continue;
    const token = `\u0000KEEP${tokens.length}\u0000`;
    tokens.push([token, literal]);
    out = out.split(literal).join(token);
  }
  out = out
    .split('\n')
    .map((line) => {
      if (line.includes('rename:keep')) return line;
      let next = line;
      for (const [from, to] of REPLACEMENTS)
        next = from instanceof RegExp ? next.replace(from, to) : next.split(from).join(to);
      return next;
    })
    .join('\n');
  for (const [token, literal] of tokens) out = out.split(token).join(literal);
  return out;
}

export function renamePath(file) {
  let next = file;
  for (const [from, to] of PATH_REPLACEMENTS) next = next.split(from).join(to);
  return next;
}

function isBinary(buffer) {
  return buffer.subarray(0, 8000).includes(0);
}

function main() {
  const files = execFileSync('git', ['ls-files', '-z'], { cwd: root, encoding: 'utf8' })
    .split('\0')
    .filter(Boolean)
    .filter((file) => !SKIP_PATHS.some((pattern) => pattern.test(file)));

  let changedFiles = 0;
  for (const file of files) {
    const absolute = path.join(root, file);
    if (!fs.existsSync(absolute) || fs.lstatSync(absolute).isSymbolicLink()) continue;
    const buffer = fs.readFileSync(absolute);
    if (isBinary(buffer)) continue;
    const before = buffer.toString('utf8');
    const fileKeep = FILE_KEEP.filter(([pattern]) => pattern.test(file)).flatMap(([, literals]) => literals);
    const after = renameText(before, [...KEEP, ...fileKeep]);
    if (after === before) continue;
    changedFiles += 1;
    if (!dryRun) fs.writeFileSync(absolute, after);
  }

  const moves = files
    .map((file) => [file, renamePath(file)])
    .filter(([from, to]) => from !== to);
  for (const [from, to] of moves) {
    if (dryRun) continue;
    fs.mkdirSync(path.dirname(path.join(root, to)), { recursive: true });
    execFileSync('git', ['mv', '-k', from, to], { cwd: root });
  }

  console.log(`${dryRun ? 'would change' : 'changed'} ${changedFiles} files, ${dryRun ? 'would move' : 'moved'} ${moves.length} paths`);
  for (const [from, to] of moves) console.log(`  ${from} -> ${to}`);
}

if (import.meta.url === `file://${process.argv[1]}`) main();
