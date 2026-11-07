#!/usr/bin/env python3
"""Real dedicated Dokploy + Rust Supervisor/worker, deterministic ACP migration.

Only accepts a freshly generated pocky-env-af lab manifest. Uses synthetic owner
credentials. No production identity, native subscription or paid model is used.
"""
import argparse, hashlib, json, os, secrets, shutil, subprocess, time, urllib.request, urllib.error
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def private(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(value)


def run(args, **kwargs):
    r = subprocess.run([str(x) for x in args], capture_output=True, **kwargs)
    if r.returncode:
        raise RuntimeError(
            "Dedicated fixture subprocess failed: "
            + Path(str(args[0])).name
            + "; output withheld"
        )
    return r.stdout.decode().strip()


def wait(fn, seconds=45):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            value = fn()
            if value:
                return value
        except (OSError, urllib.error.URLError, ValueError, KeyError):
            pass
        time.sleep(0.1)
    raise RuntimeError("Dedicated acceptance condition timed out")


class Lab:
    def __init__(self, args):
        self.args = args
        self.lab = json.loads(args.lab_manifest.read_text())
        self.prefix = self.lab["prefix"]
        assert self.prefix.startswith("pocky-env-af-")
        assert self.lab["dockerHost"].startswith(
            "unix://" + str(ROOT / ".scratch" / self.prefix) + "/"
        )
        self.docker = ["docker", "--host", self.lab["dockerHost"]]
        self.root = (
            ROOT
            / ".scratch/git-environment-feasibility/agent-first/runs"
            / (self.prefix + "-" + secrets.token_hex(3))
        )
        self.root.mkdir(parents=True)
        self.evidence = []
        self.secret_values = []
        self.token = None

    def check(self, name, value, details=None):
        assert value, "Acceptance failed: " + name
        self.evidence.append({"name": name, "passed": True, "details": details or {}})
        (self.root / "evidence.json").write_text(
            json.dumps(self.evidence, indent=2) + "\n"
        )
        print("PASS", name, flush=True)

    def api(self, path, body=None, method=None, owner=True, expected=200):
        headers = {
            "User-Agent": "pockymoe-reincarnation-fixture",
            "Content-Type": "application/json",
        }
        if owner and self.token:
            headers["Authorization"] = "Bearer " + self.token
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(
            self.base + path, data=data, headers=headers, method=method
        )
        try:
            with urllib.request.urlopen(req, timeout=65) as r:
                code = r.status
                data = r.read()
        except urllib.error.HTTPError as e:
            code = e.code
            data = e.read()
        if code != expected:
            raise RuntimeError(
                "Dedicated API " + path + " returned " + str(code) + "; body withheld"
            )
        for secret in self.secret_values:
            assert (
                secret.encode() not in data
            ), "Secret appeared in API response (value withheld)"
        return json.loads(data) if data else None

    def prepare(self):
        for name in ["home", "state", "private", "workspaces", "external", "bin"]:
            (self.root / name).mkdir(mode=0o700)
        self.project = self.root / "workspaces/original"
        self.project.mkdir()
        (self.project / "preserved.txt").write_text("keep this owner file untouched\n")
        (self.project / ".fixture").mkdir()
        shutil.copy2(
            HERE / "reincarnation_agent.py", self.project / ".fixture/handoff.py"
        )
        git = [
            "git",
            "-c",
            "user.name=Reincarnation Fixture",
            "-c",
            "user.email=fixture@test.invalid",
        ]
        run([*git, "init", self.project])
        run([*git, "-C", self.project, "add", "."])
        run([*git, "-C", self.project, "commit", "-m", "original owner baseline"])
        self.commit = run(["git", "-C", self.project, "rev-parse", "HEAD"])
        self.linked = self.root / "external/linked"
        run(
            [
                "git",
                "-C",
                self.project,
                "worktree",
                "add",
                "-b",
                "external-linked",
                self.linked,
            ]
        )
        digest = hashlib.file_digest(self.args.binary.open("rb"), "sha256").hexdigest()
        bindir = self.root / "bin" / digest
        bindir.mkdir()
        self.binary = bindir / "pockymoe"
        shutil.copy2(self.args.binary, self.binary)
        self.binary.chmod(0o555)
        docker_cli = self.root / "bin/docker"
        shutil.copy2("/usr/bin/docker", docker_cli)
        docker_cli.chmod(0o555)
        self.password = secrets.token_urlsafe(36)
        self.session_secret = secrets.token_urlsafe(48)
        self.secret_values.extend([self.password, self.session_secret])
        private(
            self.root / "private/owner.json",
            json.dumps(
                {"password": self.password, "sessionSecret": self.session_secret}
            ),
        )
        env = {
            "PATH": "/opt/pockymoe/harnesses/node_modules/.bin:/usr/local/bin:/usr/bin:/bin",
            "HOME": str(self.root / "home"),
            "POCKYMOE_DATABASE_PATH": str(self.root / "state/supervisor.sqlite"),
            "POCKYMOE_WORKSPACE_ROOT": str(self.root / "workspaces"),
            "POCKYMOE_MODE": "server",
            "POCKYMOE_HOST_MODE": "hosted",
            "POCKYMOE_HOST": "0.0.0.0",
            "POCKYMOE_PORT": "45679",
            "POCKYMOE_ADMIN_USERNAME": "fixture-owner",
            "POCKYMOE_ADMIN_PASSWORD": self.password,
            "POCKYMOE_SESSION_SECRET": self.session_secret,
            "POCKYMOE_ENABLED_AGENT_PROVIDERS": "acp,codex,claude",
            "POCKYMOE_ACP_COMMAND": "python3 .fixture/handoff.py",
            "POCKYMOE_ACP_STARTUP_TIMEOUT_MS": "20000",
            "HOST_GUARDIAN_UNRELATED": "public-unrelated-setting",
            "RUST_LOG": "warn",
            "POCKYMOE_ENVIRONMENT": "test",
            "CODEX_HOME": str(self.root / "home/.codex"),
            "CLAUDE_CONFIG_DIR": str(self.root / "home/.claude"),
        }
        private(
            self.root / "private/supervisor.env",
            "".join(k + "=" + v + "\n" for k, v in env.items()),
        )
        self.container = self.prefix + "-supervisor-" + secrets.token_hex(3)
        # The daemon's mounted repository is the only shared original storage. The
        # worker never gets this parent bind, the daemon socket or Supervisor state.
        run(
            self.docker
            + [
                "run",
                "-d",
                "--name",
                self.container,
                "--label",
                "pockymoe.testLab=" + self.prefix,
                "--cpus",
                "1",
                "--memory",
                "1536m",
                "--pids-limit",
                "512",
                "--network",
                "dokploy-network",
                "-p",
                "45679:45679",
                "--env-file",
                self.root / "private/supervisor.env",
                "--mount",
                f"type=bind,source={ROOT},target={ROOT}",
                "--mount",
                f"type=bind,source={Path(self.lab['dockerHost'].removeprefix('unix://'))},target={Path(self.lab['dockerHost'].removeprefix('unix://'))}",
                "--mount",
                f"type=bind,source={docker_cli},target=/usr/local/bin/docker,readonly",
                "--entrypoint",
                "python3",
                self.lab["nativeImage"],
                str(HERE / "supervisor_host.py"),
                "--binary",
                str(self.binary),
                "--metadata",
                str(self.root / "private/host-process.json"),
            ]
        )
        outer = json.loads(run(["docker", "inspect", self.lab["outerContainer"]]))[0]
        ip = next(iter(outer["NetworkSettings"]["Networks"].values()))["IPAddress"]
        self.base = "http://" + ip + ":45679"
        wait(
            lambda: urllib.request.urlopen(self.base + "/healthz", timeout=2).status
            == 200
        )
        auth = self.api(
            "/api/auth/login",
            {"username": "fixture-owner", "password": self.password},
            owner=False,
        )
        self.token = auth.get("token")
        # Browsers get a cookie, while API clients can use the same issued bearer.
        if not self.token:
            jar = __import__("http.cookiejar", fromlist=["CookieJar"]).CookieJar()
            opener = urllib.request.build_opener(
                urllib.request.HTTPCookieProcessor(jar)
            )
            opener.open(
                urllib.request.Request(
                    self.base + "/api/auth/login",
                    data=json.dumps(
                        {"username": "fixture-owner", "password": self.password}
                    ).encode(),
                    headers={"Content-Type": "application/json"},
                )
            ).read()
            self.token = next(iter(jar)).value
        self.secret_values.append(self.token)
        connection = json.loads(Path(self.lab["connectionFile"]).read_text())
        self.secret_values.append(connection["apiKey"])
        connection.update(
            baseUrl="http://" + self.lab["dokployContainer"] + ":3000",
            image=self.lab["nativeImage"],
            allowedRoots=[str(self.root / "workspaces"), str(self.root / "external")],
            cpuLimit=0.5,
            memoryLimitBytes=2147483648,
            pidsLimit=256,
            nofileLimit=1024,
        )
        self.connection = self.api(
            "/api/environments/connections/new", connection, "PUT"
        )
        self.api(
            "/api/environments/budget",
            {
                "maxRunning": 3,
                "maxCpu": 3,
                "maxMemoryBytes": 4 * 1024**3,
                "reservedCpu": 1,
                "reservedMemoryBytes": 512 * 1024**2,
            },
            "PUT",
        )
        self.workspace = self.api(
            "/api/workspaces",
            {"absPath": str(self.project), "label": "Original migration fixture"},
        )
        self.thread = self.api(
            "/api/threads/start",
            {
                "workspaceId": self.workspace["id"],
                "provider": "acp",
                "agentId": "custom",
                "model": "fixture",
                "reasoningEffort": "max",
                "approvalMode": "yolo",
            },
        )
        self.id = self.thread["id"]
        self.manifest = {
            "labPrefix": self.prefix,
            "runRoot": str(self.root),
            "readyCommit": self.args.ready_commit,
            "binary": str(self.binary),
            "binarySha256": digest,
            "supervisorContainer": self.container,
            "apiBaseUrl": self.base,
            "threadId": self.id,
            "workspaceId": self.workspace["id"],
            "originalPath": str(self.project),
            "linkedPath": str(self.linked),
            "baselineCommit": self.commit,
            "connectionId": self.connection["id"],
        }
        self.save()
        spec = {
            "connectionId": self.connection["id"],
            "name": "Same original thread",
            "limits": {"cpu": 0.5, "memoryBytes": 1024**3, "pids": 128, "nofile": 1024},
            "authentication": {"codex": "apiBroker", "claude": "apiBroker"},
            "egressHosts": [],
            "credentialMounts": [],
            "handoff": "Keep the owner constraint: never modify preserved.txt. Inspect original Git state before continuing. No unfinished command may be replayed.",
        }
        (self.project / ".fixture/spec.json").write_text(json.dumps(spec))
        self.check(
            "independent-server-owner-and-containerized-source-namespace-ready",
            True,
            {"supervisorContainer": self.container, "binarySha256": digest},
        )

    def save(self):
        (self.root / "manifest.json").write_text(
            json.dumps(self.manifest, indent=2) + "\n"
        )

    def main_flow(self):
        self.api(
            "/api/threads/" + self.id + "/prompt",
            {
                "prompt": "fixture: propose isolation. Owner constraint: never modify preserved.txt. Continue the original project after approval."
            },
        )
        receipt = wait(
            lambda: (
                json.loads((self.project / "proposal-receipt.json").read_text())
                if (self.project / "proposal-receipt.json").exists()
                else None
            )
        )
        self.request = receipt
        self.manifest["requestId"] = receipt["id"]
        self.save()
        self.check(
            "real-agent-cli-returns-pending-receipt-in-active-turn",
            receipt["state"] == "pending" and receipt["threadId"] == self.id,
        )
        security = wait(
            lambda: (
                json.loads((self.project / "agent-request-security.json").read_text())
                if (self.project / "agent-request-security.json").exists()
                else None
            )
        )
        for name, passed in security.items():
            self.check(name, passed)
        proof = json.loads((self.project / "controlled-host-proof.json").read_text())
        self.check("controlled-host-stdin-and-env-clear-preserved", all(proof.values()))
        wait(lambda: (self.project / "callback-stdin-eof.txt").exists())
        self.check(
            "host-acp-finite-callback-stdin-eof-preserved",
            (self.project / "callback-stdin-eof.txt").read_text() == "True",
        )
        self.check(
            "unapproved-proposal-does-not-create-or-mount-computation",
            not self.api("/api/environments")["environments"],
        )
        self.api(
            "/api/environment-requests/" + receipt["id"] + "/approve",
            {"digest": receipt["digest"]},
            owner=False,
            expected=401,
        )
        bad = {"digest": "f" * 64}
        self.api(
            "/api/environment-requests/" + receipt["id"] + "/approve", bad, expected=409
        )
        approved = self.api(
            "/api/environment-requests/" + receipt["id"] + "/approve",
            {"digest": receipt["digest"]},
        )
        again = self.api(
            "/api/environment-requests/" + receipt["id"] + "/approve",
            {"digest": receipt["digest"]},
        )
        self.check(
            "owner-approval-is-exact-digest-and-idempotent",
            approved["state"] == again["state"] == "waitingIdle",
        )
        self.check(
            "approved-active-turn-remains-on-host-without-deadlock",
            self.api("/api/threads/" + self.id)["thread"]["status"] == "running"
            and not self.api("/api/environments")["environments"],
        )
        self.manifest["oldNativeSessionId"] = self.api("/api/threads/" + self.id)[
            "thread"
        ]["providerSessionId"]
        self.save()
        (self.project / "allow-host-finish").touch()
        ready = wait(
            lambda: (
                r
                if (r := self.api("/api/environment-requests/" + receipt["id"]))[
                    "state"
                ]
                in ["ready", "failed"]
                else None
            ),
            100,
        )
        self.manifest["requestState"] = ready["state"]
        self.manifest["environmentId"] = ready.get("environmentId")
        self.save()
        self.check(
            "real-dokploy-worker-ready-after-old-domain-exits",
            ready["state"] == "ready",
            {
                "state": ready["state"],
                "errorCategory": str(ready.get("lastError", ""))[:200],
            },
        )
        result = wait(
            lambda: (
                json.loads((self.project / "continuation-evidence.json").read_text())
                if (self.project / "continuation-evidence.json").exists()
                else None
            )
        )
        for key in [
            "isolated",
            "sameThread",
            "summaryPreservedOwnerConstraint",
            "handoffUntrusted",
            "callbackWrite",
            "managedWorktree",
            "gitCommit",
            "gatewayCannotApprove",
        ]:
            self.check("continuation-" + key, result.get(key) is True)
        self.check("same-thread-continuation-once", result["continuationCount"] == 1)
        self.check(
            "owner-file-untouched",
            (self.project / "preserved.txt").read_text()
            == "keep this owner file untouched\n",
        )
        before = [
            (self.project / name).read_text()
            for name in ["host-writer-count", "callback-writer-count"]
        ]
        time.sleep(0.35)
        after = [
            (self.project / name).read_text()
            for name in ["host-writer-count", "callback-writer-count"]
        ]
        self.check("native-and-acp-callback-host-writers-stopped", before == after)
        current = self.api("/api/threads/" + self.id)["thread"]
        self.check(
            "native-session-changes-with-model-and-effort-preserved",
            current.get("providerSessionId", "").endswith(
                "/" + result["nativeSessionId"]
            )
            and current["providerSessionId"] != self.manifest["oldNativeSessionId"]
            and current.get("model") == "fixture"
            and current.get("reasoningEffort") == "max",
        )
        self.manifest["status"] = "main-flow-passed"
        self.save()

    def cleanup(self):
        # Do not remove project/state evidence. Only task-owned computation is deleted.
        for e in self.api("/api/environments")["environments"]:
            if e["state"] != "destroyed":
                self.api("/api/environments/" + e["id"] + "/destroy", {})
        run(self.docker + ["rm", "-f", self.container])
        self.manifest["cleanup"] = (
            "worker-and-test-supervisor-removed; original-project-state-retained"
        )
        self.save()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--lab-manifest", type=Path, required=True)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--ready-commit", required=True)
    p.add_argument("--retain", action="store_true")
    a = p.parse_args()
    lab = Lab(a)
    print("Evidence:", lab.root, flush=True)
    try:
        lab.prepare()
        lab.main_flow()
        lab.cleanup() if not a.retain else None
    except Exception as e:
        (lab.root / "failure.json").write_text(
            json.dumps(
                {
                    "category": type(e).__name__,
                    "message": str(e),
                    "realVsFixture": "real Dokploy/containers/Rust Supervisor/CLI/worker; deterministic ACP fixture; no paid model",
                },
                indent=2,
            )
            + "\n"
        )
        print("FAIL", type(e).__name__, str(e), "evidence", lab.root, flush=True)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
