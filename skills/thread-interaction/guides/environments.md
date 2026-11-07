# Isolated execution by owner-approved request

`pockymoe environment connections` discovers the available Dokploy connections,
resource ceilings and supported modes for your authenticated thread. Submit a
metadata-only JSON file with `pockymoe environment request --spec FILE --request-id ID`.
The command returns a durable receipt immediately; finish the current turn so the
owner-approved request can drain it and migrate. Inspect with `requests` or
`show ID`; `cancel ID` revokes a pending/waiting request before provisioning starts.

The owner reviews the immutable digest in the thread's Isolation window or
Settings. Agent/tool/yolo approval is not this owner authorization. No agent CLI
or gateway operation can approve the proposal. Approval needs an owner-signed
browser or an administrator login configured at server startup; unsigned local
HTTP and a self-bootstrapped local passcode cannot grant this authority. A different spec needs a new
request. Never include a secret value, auth-file contents or a secret environment
variable in the spec, handoff, prompt or output.

The spec accepts `connectionId`, optional `name`/`limits`, exact HTTPS DNS
`egressHosts`, `authentication` (`codex`/`claude`: `apiBroker` or `nativeFile`),
`credentialMounts` and a bounded `handoff`. The Supervisor supplies the original
thread/workspace/checkout/model/effort; they cannot be forged in the spec.

Default `apiBroker` keeps long-term API keys on the Supervisor. All other
credentials are absent by default. A credential mount names an existing absolute
`source`, absolute `target` below `/credentials`, `readOnly`, `purpose`
(`generic`, `codex`, `claude`, `github`) and optional `environment` config-path
pointers. These are direct binds of original storage, never copied credentials.
Only documented config-path variables are accepted, not arbitrary environment
values, PATH, API tokens, proxies or Pockymoe identities. Whole HOME, private
control-plane storage, special files, Docker/SSH sockets and nested/overlapping
targets are rejected. Credential directories with nested symlinks, hardlinks or
special files are unsupported; propose a narrower file/directory grant.

Native file modes require the matching mount and explicit `CODEX_HOME` or
`CLAUDE_CONFIG_DIR`. A writable directory grants the whole original config
directory, including its config/history, so the native CLI can atomically refresh
its auth file. It is exclusively writable by one Pockymoe environment until that
environment is destroyed. Other host programs are outside that lock. Read-only
single-file alternatives are `auth.json` → `/pockymoe/state/codex/auth.json` with
`CODEX_HOME=/pockymoe/state/codex`, or `.credentials.json` →
`/pockymoe/state/claude/.credentials.json` with
`CLAUDE_CONFIG_DIR=/pockymoe/state/claude`. They cannot persist refresh, and replacing
the source inode requires a new owner review. No file grant silently widens to a
parent directory. OS keyring, macOS Keychain and keyring-only gh logins are not
projected or exported. Native Codex custom-provider/profile configs fail clearly;
they cannot silently substitute an API billing route.

Native Codex requires approved `auth.openai.com` and `chatgpt.com`; native Claude
requires `api.anthropic.com` and `platform.claude.com`. Add other exact documented
destinations only when the chosen feature requires them. Network remains offline
except authenticated, generation-bound HTTPS CONNECT to approved public hosts.
SSH, arbitrary TCP/UDP, metadata/private networks and management planes are not
supported. gh needs a direct file-backed `hosts.yml` directory, `GH_CONFIG_DIR`
and the exact GitHub API/Git HTTPS hosts; its isolated Git helper invokes installed
gh, with no host global Git config or host gh environment token.

Approved requests wait for idle and admitted operations, then fence Host, end the
old harness/CLI/PTY/preview, clear the native session ID and provision Dokploy.
The same Pockymoe thread continues with its original model/effort and a fresh
native session. Its bounded journal text projection and agent handoff are context,
not extra owner authority or an LLM-generated summary. Review actual Git/files and
use the original transcript for missing instructions. Never replay an unfinished
command. Provisioning failures retain the fence and need explicit owner recovery;
there is no automatic Host fallback. The owner can explicitly destroy failed
computation and Restore Host execution while preserving original files/state.
