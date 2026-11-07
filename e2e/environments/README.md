# Real environment acceptance

This harness exercises a dedicated local Supervisor, a real Dokploy instance and
its real Docker daemon. The ACP agent is deterministic and calls only a private
host fixture upstream through the real model broker.
It does not select a mock runtime or mock lifecycle provider.

Use a fresh test lab with a `pocky-env-it-` resource prefix, loopback API URL,
dedicated Docker-in-Docker socket and a private mode-0600 connection JSON. Never
use a production Dokploy key, Docker socket, database or Supervisor. The helper
rejects Docker endpoints outside the dedicated scratch socket convention.

## Prepare without starting a Supervisor

```bash
python3 e2e/environments/verify.py prepare --connection-file /private/lab/connection.json
python3 e2e/environments/check_fixture.py
python3 e2e/environments/verify.py check-binary --binary /path/to/built/pockymoe
python3 e2e/environments/verify.py stress
```

`prepare` builds Ubuntu 24.04 with Git, Python, tini and glibc 2.39, then creates a
new Git project and an external linked worktree on the real host filesystem. The
manifest path is printed and saved in ignored
`.scratch/git-environment-feasibility/acceptance/current.json`. `--skip-build`
reuses the existing image. The build context includes only the Dockerfile and
three fixture files; no scratch or private configuration is sent to Docker.
The image's Codex/Claude command aliases are deterministic ACP fixtures. A native
Codex or Claude turn needs a separate image with the actual harness installed.

`check-binary` runs the image's actual `ldd` against a read-only bind of the given
host binary. It never executes that binary or starts a Supervisor. Repeat this
against the approved final binary when a new build is ready.

`check_fixture.py` validates ACP framing, numeric callback IDs, file and terminal
callbacks, native commands, Git commits, HTTP/WS/SSE and cancellation. This checks
the test fixture itself, and is clearly labeled separately from product evidence.

`stress` uses real Dokploy APIs to deploy three disposable network-none
containers, each with 0.125 CPU, 64 MiB and 32 pids. It verifies kernel throttling
counters, `fork` refusal and `OOMKilled`/exit 137, then deletes only these probes.
Its output and JSON evidence contain no credentials.

## Run against an explicitly approved binary

Wait for the primary implementation thread to provide its ready commit and
binary path; do not use an old target binary as an exploratory Supervisor.

```bash
python3 e2e/environments/verify.py run \
  --manifest /path/to/acceptance/manifest.json \
  --binary /path/to/ready/pockymoe \
  --ready-commit READY_COMMIT
```

After a failed run retains its resources, pass `--resume` with the next explicitly
approved build to reuse that run's connection, thread and environment IDs. Earlier
redacted evidence is archived under its `attempts/` directory. A normal new run
requires a fresh `prepare` when a manifest already contains retained workers.
If another image is building in the same small lab, `--pressure-gate /path/to/run/ready`
waits at the pressure phase for that run-local marker. Create the marker after
the image build finishes and the other computation is held; omit this option
when the lab is dedicated to this acceptance run.

To verify a Supervisor running in a different mount namespace, use the same
manifest after the main run completes:

```sh
python3 e2e/environments/containerized.py \
  --manifest /path/to/acceptance/manifest.json --ready-commit READY_COMMIT
```

For a newly approved build after the main run, also supply `--binary` explicitly.
The namespace driver pins it at a fresh SHA-derived path and runs actual `ldd`
without changing the main run's binary or evidence.

This starts a separate Supervisor container with a fresh private DB/HOME and no
published port. It mounts only the test daemon's socket into that control plane,
uses the lab's private backend network for real Dokploy API calls, and gives its
worker no control socket. The host project is mounted at a different path inside
the Supervisor. Docker inspect, inode comparison and a real command write must
prove that the provider translates this path back to the actual daemon source.
A derived test image also contains a Git project at the exact same absolute path
as a different host project; its unshared image layer must be rejected before a
Compose resource is created. A TCP Docker endpoint must be rejected explicitly
at configuration time. This last case tests real validation, without claiming a
remote daemon deployment. Only this script's worker resources and Supervisor are
cleaned after success; a failed Supervisor container is retained stopped for
diagnosis. Source files, evidence, test images and the base lab remain.

The driver checks ancestry and loader compatibility, pins each binary at a new
SHA-derived path without replacing an existing container bind, and
starts only its own `Popen` child. It constructs a new environment mapping from
scratch, with explicit `POCKYMOE_DATABASE_PATH`, `POCKYMOE_WORKSPACE_ROOT`, private
test HOME, loopback port and no production identity, proxy or API variables.
It also starts a private loopback Relay with three fictional accounts and a new
test device and connects its own Supervisor in relay mode. Local management
uses a new private test admin session; `/api/cli` uses only this Supervisor's
own private CLI credential file. Relay identities use fresh auth keys and valid
P-256 account public keys with opaque protocol vault fixtures; browser password
derivation and vault unlocking are outside this HTTP-only acceptance.
There is no `POCKYMOE_E2E_FAKE_RUNTIME` flag. Actual transport uses the custom ACP
command `python3 .fixture/acp_agent.py`, whose files live in both test checkouts.

The tests cover original and external linked storage; callback/native commands;
Git commits; CLI identity and managed worktrees; explicit command/file APIs; Web
PTY; two environment origins at the same port; HTTP/WS/SSE through the Rust
worker's TCP transport; host-file, forged-thread and host-MCP denial;
cancellation; capability revocation; stop/start/rebuild; dedicated Supervisor
restart; and destroy preserving original files and Git history. Limits are
0.25 CPU, 256 MiB and 96 pids per worker within the lab's aggregate budget.
The original thread creates a child with its actual CLI; the child must inherit
the environment and pass the same isolation identity probe. A host-only HTTP
fixture checks authenticated model SSE through the real capability gateway and
rotates its private long-term canary key without copying it into either worker.
The custom harness also attempts another harness's model profile and an invalid
endpoint. Both must be refused before any request reaches the host fixture upstream.
File routes create a real file while running, reject creation after Stop, reject
unqualified workspace requests and downloads, and refuse host-canary linked-file
stat/preview/raw access. A small generated PNG attachment must be read through
the worker while running and become unavailable after Stop.
Real Relay tests create previews through each environment's authenticated CLI,
then exercise separate same-port mappings, owner HTTP/WS/SSE, thread sharing,
unshared-account refusal, cookie separation, single-use launch tickets and
revocation of an active recipient's HTTP, WebSocket and SSE access. Preview Host
headers use the distinct public-style authority while all sockets stay loopback.
An additional host-origin fixture creates a thread preview before environment
binding. After binding, its old cookie must stop reaching the host canary and
its established WebSocket/SSE streams must close. Only the canary match boolean
and HTTP status enter evidence.
At this same binding boundary a real Host command blocks on a fixture release
file; binding must return 409 until that command finishes once. An existing Host
PTY continuously writes a progress file. The binding acknowledgement must leave
its writer stopped, reject further input through its original attachment and
preserve the host canary. A later new PTY must execute inside the worker.
An existing Host child first performs a real fixture side effect, then prevents
its parent from binding until explicitly closed. Once the parent binds, the old
child's environment lookup, reopening prompt and command must refuse execution
and leave host side effects unchanged. A new child created after binding must
inherit the environment and pass the regular isolation probe.
Both ordinary and percent-encoded dynamic thread IDs create real PTYs. Each
must report the worker's hostname and marker and refuse to read the host canary;
encoded PTY creation must fail after Stop. Percent-encoded workspace IDs also
exercise scoped file creation while running and refusal after Stop.

## Evidence, scope and cleanup

All run artifacts are ignored scratch files: `checks.json`, method/path/status
HTTP events, binary/image identity, observed stress limits and a redacted
Supervisor log. Actual API keys and generated host canaries stay in private
mode-0600 files. Fixture messages expose only selected fields and booleans; they
never echo prompts, arbitrary command output, exception text, environment dumps
or canary values. Preserve the private directory when sharing evidence.
SSE failures retain only selected framing headers, times, partial byte/event
counts and separator booleans. A raw HTTP probe records whether the terminating
chunk and EOF arrived; it never serializes the response payload or cookies.

Relay checks exercise actual HTTP protocol and authorization, with no browser UI
assertion or public DNS/TLS deployment. A passing origin or worker TCP probe is
recorded separately from that full Relay path and from real model results. CPU/PIDs/OOM
probes validate real Dokploy/Docker limits separately from worker recovery;
integration also exercises these limits inside the product-created worker.
Stopped-command checks accept a client error or an HTTP-200 command record with
`state=failed`, no exit/output and an explicit `environment_not_running` error.
A fresh marker visible to the Supervisor must remain absent, proving that the
failed command did not fall back to a host process. HTTP 200 alone never passes.
Separate hard-release and Docker exec transport-disconnect checks use detached,
double-forked writers that ignore TERM and are deliberately excluded from all
fixture cleanup. The Rust worker must kill and reap both processes, and their
files must stop changing. The transport check kills only a matching Docker exec
child beneath this run's Supervisor using a pidfd; it leaves the container up.
Further cases block the ACP input thread, ignore cancellation/TERM, and leave
the same uncooperative writers running. Stop must quiesce the old container
without waiting for an agent reply. A separate case uses a pidfd to SIGKILL only
the test Supervisor, then restarts it with its private DB. Persisted recovery
must stop the exact old container and writers before an explicit new Start;
old capabilities must fail and the blocked prompt must not replay.
Independent protocol failures remain failed checks while other probes collect
evidence. A retained child session is retried across Supervisor recovery; if it
fails, that failure stays visible while a fresh fixture child permits other
checks to continue.

Successful integration destroys its own worker environments and stops its own
Supervisor and Relay, while keeping source worktrees, commits, state and evidence.
Failure keeps worker resources for diagnosis and always stops its `Popen` children. The
parent Dokploy lab remains running until the primary completes all acceptance.
To remove this run's retained workers after diagnosis, use the pinned binary and
private database recorded by its manifest:

```sh
python3 e2e/environments/verify.py cleanup-workers \
  --manifest /path/to/acceptance/manifest.json --after-primary-done
```

This checks the saved binary hash and each environment's name and original path,
uses the actual destroy API, and stops only the Supervisor it starts. It preserves
the lab, the project files, the linked worktree and the acceptance evidence.
Never run host-wide prune, Swarm init/leave, global upgrade, or remove lab support
services from this harness.

## Agent proposals and same-thread reincarnation

The additive suite uses a new `pocky-env-af-` dedicated Dokploy 0.30.6 lab and
its private account; it never reuses the old acceptance account or an installed
Supervisor/device identity. Prepare that lab privately using the deployment
skill, then supply its non-secret manifest and the reviewed binary:

```sh
python3 e2e/environments/host_guardian.py --binary target/debug/pockymoe \
  --evidence .scratch/guardian-READY_COMMIT
python3 e2e/environments/reincarnation.py --lab-manifest /private/lab/manifest.json \
  --binary target/debug/pockymoe --ready-commit READY_COMMIT --retain
python3 e2e/environments/native_credentials.py --run-manifest /path/to/run/manifest.json
python3 e2e/environments/reincarnation_failures.py --run-manifest /path/to/run/manifest.json
```

`reincarnation.py` starts a real Rust Supervisor in a container on the original
shared storage. Its separate service-host fixture survives a Supervisor SIGKILL;
otherwise killing container PID 1 would test namespace destruction instead of
Supervisor recovery. The ACP protocol fixture submits actual CLI commands,
returns while its old turn is active, and starts native/callback writers. Real
owner approval waits for those writers and admitted operations, then the same
thread continues in the actual Dokploy worker with a new native session. It
checks original file writes, managed worktrees, Git commits, CLI identity,
model/effort, bounded owner constraints and exactly one continuation. Its
`fixture` model is protocol data and never a paid model selection.

`native_credentials.py` generates synthetic Codex/Claude/gh file stores and a
private CA. The real Codex 0.167 app-server refreshes its ChatGPT file auth; the
real Claude SDK CLI refreshes its Linux file auth and streams a deterministic
response; real gh and its Git HTTPS helper use file auth. These are **protocol
fixtures, not real subscription-service validation**. TLS leaf and root are
separate, credentials bind directly to their original directories, and evidence
contains booleans/metadata only. A public-range IP is used solely inside the
nested daemon's disposable bridge so the normal exact-host/public-IP CONNECT
checks remain active; production DNS/firewall and product code are unchanged.
The daemon, private CA, fake upstream and synthetic credentials are task-only.
The suite also checks no auth file is opened before approval, real RO failures,
source bind metadata and exclusive RW ownership.

`reincarnation_failures.py` verifies real Supervisor SIGKILL/restart, durable
pending/ready states, no replay, rejection, CLI cancellation, changed connection
failure, owner retry and explicit Host recovery. Run it after the native suite;
it changes only its own connection policy and dedicated threads. Keep at most
three live environments; destroy prior probes through the owner API when their
checks finish. Never raise the lab's limits to mask an exhausted budget.

The owner Web path uses real API calls, not route mocks:

```sh
python3 e2e/environments/prepare_owner_ui.py /path/to/run/manifest.json
POCKY_ENV_RUN_MANIFEST=/path/to/run/manifest.json corepack pnpm exec playwright test \
  --config apps/supervisor-web/e2e/environment-owner.config.ts --project desktop-chromium
```

It logs in with the generated private test owner, reviews/approves the actual CLI
proposal and checks Host PTY resize, old-domain cleanup and rejection of stale
input. It disables traces/video to avoid persisting the login. Separate
`dokploy.spec.ts --grep 'agent proposal review'` tests all three themes in both
color modes, once per explicit desktop/mobile Chromium project. Those visual
fixtures do not substitute for the real owner path above.

All drivers retain source projects, commits, private state and redacted evidence.
Destroy only environments listed in the dedicated manifest, remove its test
Supervisor/TLS containers and task network, then use that lab's scoped cleanup.
Do not delete project/state evidence or operate the production daemon resources.
