# API harness environment image

This Linux x86_64 image provides Ubuntu 24.04, Git, Python 3, CA certificates,
Node and the Codex/Claude ACP adapters. It exposes `codex`, `codex-acp`, `claude`
and `claude-agent-acp` on `/usr/local/bin`. It contains no Supervisor executable,
database, CLI home, login or API credential. The build context is this directory;
`.dockerignore` admits only the build inputs.

## Fixed inputs

Versions were checked against publisher metadata on 2026-11-07.

| Input | Version or selection | Source |
| --- | --- | --- |
| Ubuntu | 24.04, amd64 manifest digest in Dockerfile | [Official image](https://hub.docker.com/_/ubuntu) |
| Ubuntu packages | Signed snapshot `20261101T000000Z`, including updates/security | [Snapshot service](https://snapshot.ubuntu.com/) |
| Node / npm | 24.22.2 / 11.19.1, amd64 image digest in Dockerfile | [Node release metadata](https://nodejs.org/dist/v24.22.2/SHASUMS256.txt), [official image recipe](https://github.com/nodejs/docker-node/blob/main/24/bookworm/Dockerfile) |
| Codex CLI | `@openai/codex` 0.167.0 | [npm metadata](https://registry.npmjs.org/@openai/codex/0.167.0) |
| Codex ACP | `@agentclientprotocol/codex-acp` 2.5.0 | [npm metadata](https://registry.npmjs.org/@agentclientprotocol/codex-acp/2.5.0) |
| Claude ACP | `@agentclientprotocol/claude-agent-acp` 0.101.0 | [npm metadata](https://registry.npmjs.org/@agentclientprotocol/claude-agent-acp/0.101.0) |
| Claude native CLI / SDK | SDK 0.3.318 and its Linux x64 glibc native package | [SDK metadata](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/0.3.318), [native metadata](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk-linux-x64/0.3.318) |

`package-lock.json` fixes the complete npm dependency graph and SHA-512
integrities. Install with `npm ci`, including optional dependencies and published
install scripts. The image selects Linux/x64/glibc. Claude ACP requires Node 22
or newer; Node 24 satisfies it. The `claude` command uses the SDK's own native
CLI, avoiding a second large binary. The adapter resolves that same CLI from
the SDK's platform package ([source](https://github.com/agentclientprotocol/claude-agent-acp/blob/85aec26ef984a421fe8064ce43f60138a3372384/src/claude-cli.ts#L45)).

The official full Node stage supplies Node/npm and a CA bundle for the first
HTTPS apt request. Ubuntu then installs its own CA package from the fixed
snapshot. Historical Release expiry is disabled for this snapshot; archive
signature checks remain enabled. No current-mirror fallback is configured.
`verify-install.mjs` checks package versions, executable paths and native ELF
architecture without launching a harness or creating a CLI configuration.

## Build and runtime contract

Build on a dedicated test host when image construction is scheduled:

```sh
docker build --platform linux/amd64 \
  --tag pockymoe-environment-api:2026-11-07 containers/environment
```

Configure the connection with the resulting explicit image tag or digest. The
Supervisor supplies `/opt/pockymoe/bin/pockymoe` through a read-only bind and
overrides the entrypoint with `environment-worker serve`. Its provider supplies
the numeric checkout UID/GID, read-only root filesystem, `network_mode: none`,
resource limits, private writable `/pockymoe/state`, bridge bind and tmpfs. The
image defaults to UID/GID 1000 and an ordinary shell for manual inspection; it
does not run a Supervisor itself. The bound worker must be a compatible Linux
x86_64 executable; Ubuntu supplies glibc, OpenSSL 3 and the C++/atomic runtimes.

Keep both native binaries and the adapters in the immutable image. Do not run
`npx` to download an adapter at runtime. Build/version metadata checks do not
prove a native ACP conversation, model compatibility or memory/PID capacity;
verify those separately with the actual bound worker and enforced limits.

The worker sends model traffic through the Supervisor's protocol gateway. It
receives only a short-lived gateway capability. Do not pass provider keys,
mount Docker sockets or CLI homes, add privileges, or enable a host fallback.
E2B and remote storage replication are outside this image's implementation.

## Gateway configuration

The Supervisor writes the Codex provider configuration in the environment's
private `CODEX_HOME`; the image includes no `config.toml`. The accepted shape is:

```toml
model_provider = "pockymoe_environment"
[model_providers.pockymoe_environment]
name = "Pockymoe Environment"
base_url = "http://127.0.0.1:45680/model/codex"
env_key = "OPENAI_API_KEY"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
```

`OPENAI_API_KEY` names the injected short-lived capability. Codex reads it through
`env_key`; custom providers default both authentication/UI and WebSocket flags
above to false ([native source](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/model-provider-info/src/lib.rs#L145),
[configuration reference](https://developers.openai.com/codex/config-reference)).
The native client appends `/responses` to the configured base
([URL construction](https://github.com/openai/codex/blob/rust-v0.167.0/codex-rs/codex-client/src/provider.rs#L55)).
Codex ACP inherits the environment and resolves its npm Codex dependency by
default; `CODEX_PATH` is optional ([adapter source](https://github.com/agentclientprotocol/codex-acp/blob/34e08970aa324a2be985f5080674d5aa91d7666f/src/CodexJsonRpcConnection.ts#L34)).

Claude accepts `ANTHROPIC_BASE_URL=http://127.0.0.1:45680/model/claude` and the
short-lived capability in `ANTHROPIC_AUTH_TOKEN`. It sends a Bearer header and
appends `/v1/messages`; do not append `/v1` to that base. The ACP adapter spreads
`process.env` into the SDK query environment ([adapter source](https://github.com/agentclientprotocol/claude-agent-acp/blob/85aec26ef984a421fe8064ce43f60138a3372384/src/acp-agent.ts#L12184),
[official gateway documentation](https://code.claude.com/docs/en/llm-gateway-connect)).
No subscription login, `ANTHROPIC_API_KEY` duplicate or executable override is
required for this API route.

Claude ACP also loads user/project/local settings. The Supervisor's thin ACP
adapter verifies `@agentclientprotocol/claude-agent-acp` 0.101.0 and supplies the
fixed broker/capability in both SDK `options.env` and programmatic
`options.settings.env` on new, load and resume. It clears `apiKeyHelper`, OAuth
and alternate provider routing while retaining project settings sources,
permissions and tools. This matches the adapter's own programmatic route
precedence; ambient environment variables alone are insufficient. An unverified
adapter version is explicitly unsupported and never falls back to ambient env.

The image and worker set `DISABLE_AUTOUPDATER=1` and
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`. The worker clears the inherited
environment before spawning a harness, and the isolated Claude metadata pins
these values again above project settings. They
are runtime settings, not credentials or a substitute for `network_mode: none`.
The actual image binaries must still be tested against the real protocol gateway.

## Rebuild

Keep the base manifests, snapshot timestamp, Node/npm versions and npm lock in
one reviewed change. Resolve each new image tag to its Linux amd64 manifest,
review publisher release notes and update `package.json` with exact versions.
With the selected Node/npm installed, regenerate only dependency metadata using
empty configuration files:

```sh
cd containers/environment
harness_config_dir="$(mktemp -d)"
touch "$harness_config_dir/user.npmrc" "$harness_config_dir/global.npmrc"
npm install --package-lock-only --ignore-scripts --no-audit --no-fund \
  --registry=https://registry.npmjs.org \
  --userconfig="$harness_config_dir/user.npmrc" \
  --globalconfig="$harness_config_dir/global.npmrc"
rm -rf "$harness_config_dir"
```

Use an approved build host to build and run acceptance afterward. Preserve the
selected inputs in an artifact archive if rebuilding beyond Ubuntu Snapshot's
published retention period is required.
