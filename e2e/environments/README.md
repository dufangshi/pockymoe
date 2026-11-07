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
Real Relay tests create previews through each environment's authenticated CLI,
then exercise separate same-port mappings, owner HTTP/WS/SSE, thread sharing,
unshared-account refusal, cookie separation, single-use launch tickets and
revocation of an active recipient's HTTP, WebSocket and SSE access. Preview Host
headers use the distinct public-style authority while all sockets stay loopback.
An additional host-origin fixture creates a thread preview before environment
binding. After binding, its old cookie must stop reaching the host canary and
its established WebSocket/SSE streams must close. Only the canary match boolean
and HTTP status enter evidence.

## Evidence, scope and cleanup

All run artifacts are ignored scratch files: `checks.json`, method/path/status
HTTP events, binary/image identity, observed stress limits and a redacted
Supervisor log. Actual API keys and generated host canaries stay in private
mode-0600 files. Fixture messages expose only selected fields and booleans; they
never echo prompts, arbitrary command output, exception text, environment dumps
or canary values. Preserve the private directory when sharing evidence.

Relay checks exercise actual HTTP protocol and authorization, with no browser UI
assertion or public DNS/TLS deployment. A passing origin or worker TCP probe is
recorded separately from that full Relay path and from real model results. CPU/PIDs/OOM
probes validate real Dokploy/Docker limits separately from worker recovery;
integration also exercises these limits inside the product-created worker.
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
