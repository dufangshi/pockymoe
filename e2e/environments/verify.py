#!/usr/bin/env python3
"""Real Dokploy environment acceptance; never start an unapproved Supervisor."""
import argparse
import base64
import hashlib
import http.server
import json
import os
import re
import secrets
import shlex
import shutil
import signal
import socket
import socketserver
import stat
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
import zlib
from pathlib import Path
sys.dont_write_bytecode = True
from wire import WebSocket

HERE = Path(__file__).resolve().parent
REPOSITORY = HERE.parent.parent
SCRATCH = REPOSITORY / ".scratch/git-environment-feasibility/acceptance"
MARKER = "POCKY_ENV_RESULT "


class Failure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


def private_write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.fchmod(fd, 0o600)
    with os.fdopen(fd, "w") as file:
        file.write(value)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def sha256_file(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def basic_env(home):
    # Deliberately do not copy os.environ: no production device, relay, agent,
    # shell proxy, API credential, CLI config or harness identity is inherited.
    return {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(home), "USER": "fixture",
            "LANG": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1", "GIT_CONFIG_NOSYSTEM": "1"}


def process(argv, cwd=None, env=None, timeout=60, input=None, checked=True):
    result = subprocess.run([str(x) for x in argv], cwd=cwd, env=env, input=input,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)
    if checked:
        require(result.returncode == 0, "Subprocess failed: " + Path(str(argv[0])).name)
    return result


def git(cwd, *args):
    result = process(["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
        "-c", "user.name=Environment Fixture", "-c", "user.email=fixture@test.invalid", *args],
        cwd=cwd, env=basic_env(cwd))
    return result.stdout.decode().strip()


def unused_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class Evidence:
    def __init__(self, root, secret_values=()):
        self.root = root
        self.checks = []
        self.secrets = [x for x in secret_values if x]
        self.lock = threading.Lock()

    def redact(self, data):
        for value in self.secrets:
            data = data.replace(value, "[REDACTED]")
        return data

    def check(self, name, passed, detail=None):
        entry = {"check": name, "passed": bool(passed)}
        if detail is not None:
            entry["detail"] = detail
        with self.lock:
            self.checks.append(entry)
            write_json(self.root / "checks.json", self.checks)
        return bool(passed)

    def event(self, method, path, status):
        with self.lock, (self.root / "http-events.jsonl").open("a") as file:
            file.write(json.dumps({"method": method, "path": path, "status": status}) + "\n")

    def audit_public_files(self):
        scanned = matched = 0
        for base in [self.root, HERE]:
            for path in base.rglob("*"):
                relative = path.relative_to(base)
                if any(part in {"private", "state", "home", "bin", "unauthorized", "__pycache__"}
                       for part in relative.parts[:-1]):
                    continue
                if not path.is_file() or path.is_symlink() or path.stat().st_size > 4 * 1024 * 1024:
                    continue
                if path.suffix not in {".json", ".jsonl", ".log", ".md", ".txt", ".py"} and path.name != "Dockerfile":
                    continue
                text = path.read_text(errors="replace")
                scanned += 1
                if any(value in text for value in self.secrets):
                    matched += 1
                    path.write_text(self.redact(text))
        self.check("public-evidence-and-fixture-credentials-absent", matched == 0,
            {"filesScanned": scanned, "filesRequiringRedaction": matched})


class API:
    def __init__(self, base, evidence, key=None):
        parsed = urllib.parse.urlsplit(base)
        require(parsed.hostname == "127.0.0.1", "Acceptance API must use an explicit loopback URL")
        self.base = base.rstrip("/")
        self.evidence = evidence
        self.key = key
        self.bearer = None
        self.cli_bearer = None
        self.opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, path, body=None, method=None, allowed=(200,), raw=False):
        headers = {"User-Agent": "pockymoe-environment-e2e/1.0", "Content-Type": "application/json"}
        if self.key:
            headers["x-api-key"] = self.key
        bearer = self.cli_bearer if path == "/api/cli" and self.cli_bearer else self.bearer
        if bearer:
            headers["Authorization"] = "Bearer " + bearer
        request = urllib.request.Request(self.base + path, data=None if body is None else json.dumps(body).encode(),
            method=method, headers=headers)
        try:
            with self.opener.open(request, timeout=120) as response:
                data = response.read(32 * 1024 * 1024)
                status = response.status
        except urllib.error.HTTPError as error:
            status = error.code
            data = error.read(1024 * 1024)
        self.evidence.event(request.get_method(), path, status)
        if status not in allowed:
            try:
                error_value = json.loads(data)
                detail = {key: error_value[key] for key in ["code", "message"] if key in error_value}
                with (self.evidence.root / "http-errors.jsonl").open("a") as output:
                    output.write(self.evidence.redact(json.dumps({"method": request.get_method(), "path": path, "status": status, "detail": detail})) + "\n")
            except (ValueError, TypeError):
                pass
        require(status in allowed, f"HTTP {request.get_method()} {path} returned {status}")
        if raw:
            return status, data
        try:
            return status, json.loads(data) if data else None
        except ValueError:
            return status, None

    def __call__(self, path, body=None, method=None):
        return self.request(path, body, method)[1]


class ModelUpstream:
    """Private host-only upstream; never logs an Authorization header or body."""
    def __init__(self, root, evidence):
        self.root, self.evidence = root, evidence
        self.secrets = [secrets.token_urlsafe(48), secrets.token_urlsafe(48)]
        for index, value in enumerate(self.secrets):
            private_write(root / f"private/model-canary-{index + 1}", value)
        evidence.secrets.extend(self.secrets)
        self.revision = 0
        self.requests = []
        upstream = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_):
                pass

            def do_POST(self):
                size = int(self.headers.get("Content-Length", "0"))
                payload = json.loads(self.rfile.read(min(size, 1048576)))
                authenticated = self.headers.get("Authorization") == "Bearer " + upstream.secrets[upstream.revision]
                accepted = authenticated and self.path == "/v1/responses" and payload.get("model") == "fixture" and payload.get("stream") is True
                upstream.requests.append({"revision": upstream.revision, "authenticated": authenticated, "accepted": accepted})
                self.send_response(200 if accepted else 403)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                if accepted:
                    for event in [{"type": "response.output_text.delta", "delta": "environment-fixture-delta"},
                            {"type": "response.completed", "response": {"id": "fixture-response", "status": "completed"}}]:
                        self.wfile.write(("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n").encode())
                        self.wfile.flush()
                        time.sleep(0.4)
                self.close_connection = True

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def activate(self, api):
        profile = api("/api/management/upstreams", {"id": "", "name": "Private environment fixture " + str(self.revision),
            "harness": "codex", "baseUrl": f"http://127.0.0.1:{self.server.server_port}/v1",
            "apiKey": self.secrets[self.revision], "authType": "api_key", "model": "fixture", "apiType": "responses"})
        self.evidence.check("upstream-canary-write-only-" + str(self.revision), self.secrets[self.revision] not in json.dumps(profile))
        api("/api/management/upstreams/" + profile["id"], {"action": "activate"})

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)


def model_secret_checks(root, state, connection, environments, api, model_upstream, evidence):
    # Fetch files into memory and compare on the host. The canary itself is
    # never passed to the worker as an argument, environment value or file.
    reader = """import os,sys
budget=32*1024*1024
for root in ['/pockymoe/state',os.getcwd()]:
 for folder,dirs,files in os.walk(root,followlinks=False):
  dirs[:]=[d for d in dirs if not os.path.islink(os.path.join(folder,d))]
  for name in files:
   path=os.path.join(folder,name)
   if os.path.islink(path): continue
   try:
    with open(path,'rb') as source:
     chunk=source.read(min(budget,1048576))
    sys.stdout.buffer.write(chunk);budget-=len(chunk)
   except OSError: pass
   if budget<=0:sys.exit(0)
"""
    for index, environment in enumerate(environments):
        container = environment["containerId"]
        detail = json.loads(docker_command(connection, "inspect", container).stdout)[0]
        files = docker_command(connection, "exec", "-w", environment["workspacePath"], container, "python3", "-c", reader).stdout
        logs = docker_command(connection, "logs", container).stdout
        snapshot = json.dumps(api(f"/api/threads/{environment['threadId']}"))
        values = [value.encode() for value in model_upstream.secrets]
        evidence.check("model-canary-absent-from-worker-env-files-logs-snapshot-" + str(index),
            all(value not in files and value not in logs and value not in json.dumps(detail).encode()
                and value not in snapshot.encode() for value in values))
    history = b"".join(path.read_bytes() for path in (root / "workspaces").rglob("*history*") if path.is_file())
    evidence.check("model-canary-absent-from-project-history", all(value.encode() not in history for value in model_upstream.secrets))


def docker_command(connection, *args, **kwargs):
    host = connection.get("dockerHost", "")
    require(host.startswith("unix://") and "/.scratch/pocky-env-it-" in host,
            "Only the explicitly isolated test Docker socket is supported")
    return process(["docker", "--host", host, *args], env=basic_env(SCRATCH), **kwargs)


def load_state(path):
    path = (path or SCRATCH / "current.json").resolve()
    state = json.loads(path.read_text())
    require(state["prefix"].startswith("pocky-env-it-"), "Invalid test resource prefix")
    root = Path(state["runRoot"]).resolve()
    require(root.is_relative_to(SCRATCH.resolve()), "Run root must stay inside dedicated scratch")
    return state, root


def load_connection(path):
    path = path.resolve()
    require(stat.S_IMODE(path.stat().st_mode) == 0o600, "Connection file must be mode 0600")
    config = json.loads(path.read_text())
    require(config.get("apiKey"), "Private connection must contain a lab API key")
    require(config.get("name", "").startswith("pocky-env-it-"), "Only a dedicated test lab is allowed")
    return config


def prepare(args):
    connection = load_connection(args.connection_file)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    prefix = connection["name"] + "-accept-" + secrets.token_hex(3)
    root = SCRATCH / prefix
    root.mkdir(mode=0o700)
    for name in ["private", "home", "state", "bin", "workspaces", "external", "unauthorized"]:
        (root / name).mkdir(mode=0o700)
    original = root / "workspaces" / "original"
    original.mkdir()
    fixture = original / ".fixture"
    fixture.mkdir()
    for name in ["acp_agent.py", "fixture_service.py"]:
        shutil.copyfile(HERE / name, fixture / name)
    (original / "tracked.txt").write_text("initial\n")
    git(original, "init", "-q", "-b", "main")
    git(original, "add", ".")
    git(original, "commit", "-qm", "Environment acceptance baseline")
    linked = root / "external" / "linked"
    git(original, "worktree", "add", "-q", "-b", prefix + "-linked", str(linked))
    canary = secrets.token_urlsafe(36)
    private_write(root / "private/canary", canary)
    private_write(root / "unauthorized/host-canary", canary)
    for path in [original, linked]:
        os.symlink(root / "unauthorized/host-canary", path / "escape-to-host")
    # The full private connection stays outside the original/linked bind roots.
    private_write(root / "private/connection.json", json.dumps(connection) + "\n")
    state = {"prefix": prefix, "runRoot": str(root), "connectionFile": str(root / "private/connection.json"),
        "originalPath": str(original), "linkedPath": str(linked), "hostCanaryPath": str(root / "unauthorized/host-canary"),
        "databasePath": str(root / "state/supervisor.sqlite"), "homePath": str(root / "home"),
        "port": unused_port(), "image": args.image or connection["name"] + "-worker:ubuntu24",
        "fixtureBaselineCommit": git(original, "rev-parse", "HEAD"),
        "linkedGitPointerSha256": sha256_file(linked / ".git"),
        "status": "prepared-waiting-for-ready-binary"}
    write_json(root / "manifest.json", state)
    write_json(SCRATCH / "current.json", state)
    evidence = Evidence(root)
    if not args.skip_build:
        built = docker_command(connection, "build", "--memory", "512m", "--cpu-period", "100000", "--cpu-quota", "100000",
            "--tag", state["image"], str(HERE), timeout=300)
        (root / "image-build.log").write_bytes(built.stdout + built.stderr)
    inspected = json.loads(docker_command(connection, "image", "inspect", state["image"]).stdout)[0]
    write_json(root / "image.json", {"image": state["image"], "id": inspected["Id"], "size": inspected["Size"]})
    evidence.check("real-image-built", True, {"image": state["image"], "id": inspected["Id"]})
    print(json.dumps({"status": state["status"], "manifest": str(root / "manifest.json"), "image": state["image"]}))


def check_binary(args, state=None, root=None):
    if state is None:
        state, root = load_state(args.manifest)
    connection = load_connection(Path(state["connectionFile"]))
    binary = args.binary.resolve(strict=True)
    require(binary.is_relative_to(REPOSITORY), "Binary must be visible within the lab's identical host workspace mount")
    # ldd checks loader compatibility without launching any Supervisor.
    result = docker_command(connection, "run", "--rm", "--name", state["prefix"] + "-ldd",
        "--label", "pockymoe.acceptance=" + state["prefix"], "--network", "none", "--read-only", "--user", "1000:1000",
        "--cpus", ".25", "--memory", "128m", "--pids-limit", "32", "--cap-drop", "ALL",
        "--security-opt", "no-new-privileges:true", "--mount", f"type=bind,source={binary},target=/opt/pockymoe/bin/pockymoe,readonly",
        "--entrypoint", "ldd", state["image"], "/opt/pockymoe/bin/pockymoe", checked=False)
    output = (result.stdout + result.stderr).decode(errors="replace")
    passed = result.returncode == 0 and "not found" not in output and "version `" not in output
    evidence = {"passed": passed, "binary": str(binary), "sha256": sha256_file(binary),
        "image": state["image"], "ldd": output}
    write_json(root / "binary-compatibility.json", evidence)
    require(passed, "Current host Rust binary does not load in the test image; see binary-compatibility.json")
    print(json.dumps({"binaryCompatible": True, "evidence": str(root / "binary-compatibility.json")}))
    return binary


class Supervisor:
    def __init__(self, state, root, binary, evidence, relay_config=None):
        self.state, self.root, self.binary, self.evidence = state, root, binary, evidence
        self.process = None
        self.log = None
        self.base = f"http://127.0.0.1:{state['port']}"
        self.api = API(self.base, evidence)
        self.relay_config = relay_config

    def start(self):
        self.api.bearer = None
        self.api.cli_bearer = None
        environment = basic_env(self.root / "home")
        environment.update({"POCKYMOE_DATABASE_PATH": self.state["databasePath"],
            "POCKYMOE_WORKSPACE_ROOT": str(self.root / "workspaces"), "POCKYMOE_MODE": "local",
            "POCKYMOE_HOST": "127.0.0.1", "POCKYMOE_PORT": str(self.state["port"]),
            "POCKYMOE_HOST_MODE": "hosted", "POCKYMOE_ENABLED_AGENT_PROVIDERS": "acp,codex,claude",
            "POCKYMOE_ACP_COMMAND": "python3 .fixture/acp_agent.py", "POCKYMOE_ACP_STARTUP_TIMEOUT_MS": "15000",
            "POCKY_ENV_FIXTURE_SELF_CHECK_BINARY": str(self.binary),
            "POCKY_ENV_HOST_CANARY": (self.root / "private/canary").read_text(),
            "CODEX_HOME": str(self.root / "home/.codex"), "CLAUDE_CONFIG_DIR": str(self.root / "home/.claude"),
            "RUST_LOG": "warn", "POCKYMOE_ENVIRONMENT": "test"})
        if self.relay_config:
            auth_path = self.root / "private/supervisor-auth.json"
            if not auth_path.exists():
                private_write(auth_path, json.dumps({"password": secrets.token_urlsafe(36), "sessionSecret": secrets.token_urlsafe(48)}) + "\n")
            auth = json.loads(auth_path.read_text())
            self.evidence.secrets.extend([auth["password"], auth["sessionSecret"]])
            environment.update({"POCKYMOE_HOST_MODE": "standalone", "POCKYMOE_RELAY_SERVER_URL": self.relay_config["baseUrl"],
                "POCKYMOE_RELAY_AGENT_TOKEN": self.relay_config["agentToken"],
                "POCKYMOE_RELAY_SUPERVISOR_HOST": "127.0.0.1", "POCKYMOE_RELAY_SUPERVISOR_PORT": str(self.state["port"]),
                "POCKYMOE_ADMIN_USERNAME": "fixture-supervisor", "POCKYMOE_ADMIN_PASSWORD": auth["password"],
                "POCKYMOE_SESSION_SECRET": auth["sessionSecret"]})
        log_path = self.root / "private/supervisor.log"
        fd = os.open(log_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        self.log = os.fdopen(fd, "ab", buffering=0)
        self.process = subprocess.Popen([str(self.binary), "relay-supervisor" if self.relay_config else "supervisor"], cwd=self.root,
            env=environment, stdout=self.log, stderr=self.log, start_new_session=True)
        private_write(self.root / "private/supervisor.pid", str(self.process.pid))
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            require(self.process.poll() is None, "Dedicated test Supervisor exited; inspect its private log locally")
            try:
                identity_path = Path(self.state["databasePath"]).with_suffix(".cli.json")
                if identity_path.exists():
                    identity = json.loads(identity_path.read_text())
                    self.api.cli_bearer = identity["token"]
                    if identity["token"] not in self.evidence.secrets:
                        self.evidence.secrets.append(identity["token"])
                self.api("/healthz")
                if self.relay_config and not self.api.bearer:
                    session = self.api("/api/auth/login", {"username": "fixture-supervisor", "password": auth["password"]})
                    self.api.bearer = session["token"]
                    self.evidence.secrets.append(session["token"])
                self.api("/api/environments")
                return
            except (OSError, Failure):
                time.sleep(0.15)
        raise Failure("Dedicated test Supervisor did not expose the new environment API")

    def stop(self):
        if self.process and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=3)
        if self.log:
            self.log.close()
            self.log = None
        self.process = None

    def kill_for_recovery(self):
        require(self.process and self.process.poll() is None, "Owned test Supervisor is not running")
        descriptor = os.pidfd_open(self.process.pid)
        try:
            # Deliberately kill only our Popen Supervisor, leaving transports
            # and uncooperative writers for persisted recovery to reclaim.
            signal.pidfd_send_signal(descriptor, signal.SIGKILL)
        finally:
            os.close(descriptor)
        self.process.wait(timeout=3)
        self.log.close()
        self.log = None
        self.process = None

    def command(self, thread, argv, cwd=".", request_id=None):
        return self.api("/api/cli", {"operation": "commandRun", "threadId": thread, "input": {
            "argv": argv, "cwd": cwd, "timeoutSeconds": 30, "clientRequestId": request_id}})

    def prompt(self, thread, case, wait=True):
        # Correlate exact public request IDs, independent of transcript page
        # sizes or an earlier attempt's same-named fixture result.
        case = dict(case, caseId=uuid.uuid4().hex)
        self.api(f"/api/threads/{thread}/prompt", {"prompt": "POCKY_ENV_CASE " + json.dumps(case), "clientRequestId": case["caseId"]})
        if not wait:
            return None
        deadline = time.monotonic() + 50
        while time.monotonic() < deadline:
            detail = self.api(f"/api/threads/{thread}")
            results = find_results(detail)
            matched = [r for r in results if r.get("case") == case["case"] and r.get("caseId") == case["caseId"]]
            own_turns = [turn for turn in detail.get("turns", []) if case["caseId"] in json.dumps(turn)]
            if matched and any(turn.get("status") == "completed" for turn in own_turns):
                return matched[-1]
            errors = [r for r in results if r.get("case") == "fixture-error" and r.get("caseId") == case["caseId"]]
            if errors:
                raise Failure("Fixture case error: " + case["case"] + " (" + errors[-1].get("failureType", "unknown") + ")")
            if any(turn.get("status") == "failed" for turn in own_turns):
                raise Failure("Fixture turn failed: " + case["case"])
            time.sleep(0.2)
        raise Failure("Fixture turn result not observed: " + case["case"])


class WorkerForwarder:
    """Probe Rust's worker TCP data path without requiring a public Relay."""
    def __init__(self, connection, container, port):
        host = connection["dockerHost"]
        class Handler(socketserver.BaseRequestHandler):
            def handle(self):
                child = subprocess.Popen(["docker", "--host", host, "exec", "-i", container,
                    "/opt/pockymoe/bin/pockymoe", "environment-worker", "tcp", str(port)],
                    env=basic_env(SCRATCH), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                def inbound():
                    try:
                        while True:
                            data = self.request.recv(65536)
                            if not data:
                                break
                            child.stdin.write(data)
                            child.stdin.flush()
                    except (OSError, ValueError):
                        pass
                    finally:
                        try:
                            child.stdin.close()
                        except OSError:
                            pass
                reader = threading.Thread(target=inbound, daemon=True)
                reader.start()
                try:
                    while True:
                        data = os.read(child.stdout.fileno(), 65536)
                        if not data:
                            break
                        self.request.sendall(data)
                except OSError:
                    pass
                finally:
                    try:
                        self.request.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    child.terminate()
                    try:
                        child.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=3)
        self.server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
        self.server.daemon_threads = True
        self.port = self.server.server_address[1]

    def __enter__(self):
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self.port

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()


def find_results(value):
    results = []
    if isinstance(value, dict):
        for item in value.values():
            results.extend(find_results(item))
    elif isinstance(value, list):
        for item in value:
            results.extend(find_results(item))
    elif isinstance(value, str) and MARKER in value:
        for chunk in value.split(MARKER)[1:]:
            try:
                parsed, _ = json.JSONDecoder().raw_decode(chunk)
                results.append(parsed)
            except ValueError:
                pass
    return results


def check_detached_cleanup(supervisor, thread_id, environment, connection, path, evidence, transport=False):
    # Release first so the next fixture process definitely belongs to the
    # tested thread, and is the newest matching private Docker exec transport.
    supervisor.api(f"/api/threads/{thread_id}/release", {})
    result = supervisor.prompt(thread_id, {"case": "detached-writers"})
    require(result.get("started") and result.get("fixtureWillNotClean"), "Uncooperative fixture writers did not start")
    pids = result["pids"]
    if transport:
        candidates = []
        process_rows = {}
        process_starts = {}
        for directory in Path("/proc").iterdir():
            if not directory.name.isdigit():
                continue
            try:
                fields = (directory / "stat").read_text().rsplit(")", 1)[1].split()
                process_rows[int(directory.name)] = int(fields[1])
                process_starts[int(directory.name)] = fields[19]
            except (OSError, ValueError, IndexError):
                continue
        descendants = {supervisor.process.pid}
        for _ in range(8):
            descendants.update(pid for pid, parent in process_rows.items() if parent in descendants)
        for pid in descendants - {supervisor.process.pid}:
            try:
                arguments = (Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
            except OSError:
                continue
            if (arguments and Path(os.fsdecode(arguments[0])).name == "docker" and b"exec" in arguments
                    and environment["containerId"].encode() in arguments and b"environment-worker" in arguments
                    and (b"process" in arguments or b"run" in arguments)):
                candidates.append(pid)
        require(candidates, "Private Docker exec transport was not located under the owned Supervisor")
        target_pid = max(candidates)
        descriptor = os.pidfd_open(target_pid)
        try:
            current_start = (Path("/proc") / str(target_pid) / "stat").read_text().rsplit(")", 1)[1].split()[19]
            require(current_start == process_starts[target_pid], "Owned Docker exec process identity changed before transport fault")
            signal.pidfd_send_signal(descriptor, signal.SIGKILL)
        finally:
            os.close(descriptor)
    else:
        supervisor.api(f"/api/threads/{thread_id}/release", {})
    remaining = None
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        probe = docker_command(connection, "exec", environment["containerId"], "python3", "-c",
            "import json,pathlib,sys;print(json.dumps([p for p in json.loads(sys.argv[1]) if pathlib.Path('/proc/'+str(p)).exists()]))", json.dumps(pids))
        remaining = json.loads(probe.stdout)
        if not remaining:
            break
        time.sleep(0.2)
    prefix = "transport-disconnect" if transport else "hard-release"
    reaped = evidence.check(prefix + "-reaps-double-fork-term-ignoring-writers", remaining == [], {"remainingPidCount": len(remaining or [])})
    progress = [Path(path) / ("detached-progress-" + str(pid) + ".txt") for pid in pids]
    before = [file.read_text() if file.exists() else None for file in progress]
    time.sleep(0.4)
    stopped = evidence.check(prefix + "-no-unmanaged-writer-continues", before == [file.read_text() if file.exists() else None for file in progress])
    require(reaped and stopped, prefix + " left an uncooperative native writer; retaining the test container for diagnosis")


def check_host_mcp_rejected(supervisor, thread_id, root, evidence):
    marker = root / "unauthorized/host-mcp-marker"
    marker.unlink(missing_ok=True)
    name = "fixture-host-marker"
    parent = supervisor.api(f"/api/threads/{thread_id}")["thread"]
    supervisor.api("/api/cli", {"operation": "mcpSet", "server": {"name": name, "transport": "stdio", "command": "/bin/sh",
        "args": ["-c", "printf observed > " + shlex.quote(str(marker))], "agents": ["custom"], "enabled": True}})
    try:
        status, response = supervisor.api.request("/api/threads/start", {"workspaceId": parent["workspaceId"],
            "parentThreadId": thread_id, "provider": "acp", "agentId": "custom", "model": "fixture",
            "title": "Host MCP rejection fixture", "approvalMode": "yolo"}, allowed=(200, 400, 409))
        rejected = status >= 400 and "unsupported_environment_mcp" in json.dumps(response)
        evidence.check("host-shell-mcp-registry-rejected-on-new-isolated-acp-load", rejected)
        evidence.check("host-shell-mcp-marker-never-executed", not marker.exists())
        if status == 200:
            supervisor.api(f"/api/threads/{response['id']}/release", {})
    finally:
        supervisor.api("/api/cli", {"operation": "mcpRemove", "name": name})


def fixture_png():
    def chunk(kind, data):
        return struct.pack("!I", len(data)) + kind + data + struct.pack("!I", zlib.crc32(kind + data))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack("!IIBBBBB", 1, 1, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes([0, 64, 128, 192, 255]))) + chunk(b"IEND", b"")


def encoded_id(value):
    return "%" + format(ord(value[0]), "02X") + value[1:]


def check_worker_pty(supervisor, thread_id, path, container, connection, evidence, host_canary_path, label, encoded=False):
    route_id = encoded_id(thread_id) if encoded else thread_id
    prefix = label + ("-encoded-thread-id" if encoded else "")
    marker = Path(path) / ("pty-identity-" + uuid.uuid4().hex + ".json")
    created = supervisor.api(f"/api/threads/{route_id}/shell", {"cols": 80, "rows": 24, "label": "Environment acceptance"})
    shell_id = created["activeShellId"]
    try:
        with WebSocket(supervisor.base.replace("http://", "ws://") + "/ws",
                {"Authorization": "Bearer " + supervisor.api.bearer}) as websocket:
            websocket.send_json({"type": "shell.attach", "shellId": shell_id, "cols": 80, "rows": 24})
            viewer = None
            for _ in range(30):
                message = json.loads(websocket.receive())
                if message.get("type") == "shell.connected":
                    viewer = message["payload"]["viewerId"]
                    break
            require(viewer, "PTY viewer did not attach")
            code = ("import json,os\nfrom pathlib import Path\nreadable=False\ntry:\n Path(" + repr(str(host_canary_path))
                + ").read_bytes();readable=True\nexcept OSError:pass\nPath(" + repr(marker.name)
                + ").write_text(json.dumps({'hostname':Path('/etc/hostname').read_text().strip(),"
                "'workerMarked':os.environ.get('POCKYMOE_ENVIRONMENT_WORKER')=='1','hostCanaryReadable':readable}))\n")
            websocket.send_json({"type": "shell.input", "shellId": shell_id, "viewerId": viewer,
                "data": "python3 -c " + shlex.quote(code) + "\n"})
            deadline = time.monotonic() + 6
            while (not marker.exists() or not marker.stat().st_size) and time.monotonic() < deadline:
                time.sleep(.1)
            expected_hostname = json.loads(docker_command(connection, "inspect", container).stdout)[0]["Config"]["Hostname"]
            observed = json.loads(marker.read_text()) if marker.exists() and marker.stat().st_size else {}
            evidence.check(prefix + "-web-pty-inside-environment", observed.get("hostname") == expected_hostname
                and observed.get("workerMarked") is True and observed.get("hostCanaryReadable") is False,
                {"hostnameMatchesWorker": observed.get("hostname") == expected_hostname, "workerMarked": observed.get("workerMarked"),
                 "hostCanaryReadable": observed.get("hostCanaryReadable"), "encodedDynamicPath": encoded})
    finally:
        supervisor.api(f"/api/shells/{shell_id}/terminate", {})


def check_web_file_routes(supervisor, thread_id, path, evidence, host_canary_path, prefix, running):
    thread = supervisor.api(f"/api/threads/{thread_id}")["thread"]
    workspace_id = thread["workspaceId"]
    api = supervisor.api
    allowed = (200, 201, 400, 403, 404, 409, 422, 503)
    relative_image = f".temp/threads/{thread_id}/fixture-pixel.png"
    image = Path(path) / relative_image
    if running:
        image.parent.mkdir(parents=True, exist_ok=True)
        image.write_bytes(fixture_png())
    scoped_files = f"/api/workspaces/{workspace_id}/files?threadId={thread_id}"
    marker = Path(path) / (("web-created-" if running else "stopped-create-") + uuid.uuid4().hex + ".txt")
    status, _ = api.request(scoped_files, {"path": marker.name}, allowed=allowed)
    evidence.check(prefix + "-web-create-" + ("inside-running-worker" if running else "stopped-refused-without-host-write"),
        status == 201 and marker.is_file() if running else status in {400, 403, 409, 503} and not marker.exists(), {"httpStatus": status})
    encoded_marker = Path(path) / ("encoded-workspace-create-" + uuid.uuid4().hex + ".txt")
    status, _ = api.request(f"/api/workspaces/{encoded_id(workspace_id)}/files?threadId={thread_id}", {"path": encoded_marker.name}, allowed=allowed)
    evidence.check(prefix + "-encoded-workspace-create-" + ("inside-running-worker" if running else "stopped-refused-without-host-write"),
        status == 201 and encoded_marker.is_file() if running else status in {400, 403, 409, 503} and not encoded_marker.exists(), {"httpStatus": status})
    if not running:
        status, created = api.request(f"/api/threads/{encoded_id(thread_id)}/shell", {"cols": 80, "rows": 24}, allowed=allowed)
        evidence.check(prefix + "-encoded-thread-id-pty-stopped-refused", status in {400, 403, 409, 503}, {"httpStatus": status})
        if status == 200 and isinstance(created, dict) and created.get("activeShellId"):
            api(f"/api/shells/{created['activeShellId']}/terminate", {})
    status, payload = api.request(f"/api/threads/{thread_id}/assets/image?" + urllib.parse.urlencode({"path": relative_image}), allowed=allowed, raw=True)
    evidence.check(prefix + "-thread-image-" + ("read-by-worker" if running else "stopped-refused"),
        status == 200 and payload == fixture_png() if running else status in {400, 403, 409, 503} and payload != fixture_png(), {"httpStatus": status})
    status, payload = api.request(f"/api/workspaces/{workspace_id}/files/download?" + urllib.parse.urlencode({"threadId": thread_id, "path": "tracked.txt"}), allowed=allowed, raw=True)
    evidence.check(prefix + "-workspace-download-explicitly-unsupported", status in {400, 403, 409, 503}
        and (not running or b"unsupported_environment" in payload and b"download" in payload), {"httpStatus": status})
    canary = Path(host_canary_path).read_bytes()
    for action in ["stat", "preview", "raw"]:
        status, payload = api.request(f"/api/threads/{thread_id}/linked-files/{action}?" + urllib.parse.urlencode({"path": host_canary_path}), allowed=allowed, raw=True)
        evidence.check(prefix + "-linked-host-file-" + action + "-denied", status in {400, 403, 409, 503} and canary not in payload,
            {"httpStatus": status, "hostCanaryObserved": canary in payload})
    status, _ = api.request(f"/api/workspaces/{workspace_id}/files/tree?path=.", allowed=allowed)
    evidence.check(prefix + "-unqualified-workspace-tree-denied", status in {400, 403, 409}, {"httpStatus": status})
    unqualified = Path(path) / ("unqualified-create-" + uuid.uuid4().hex + ".txt")
    status, _ = api.request(f"/api/workspaces/{workspace_id}/files", {"path": unqualified.name}, allowed=allowed)
    evidence.check(prefix + "-unqualified-workspace-create-denied", status in {400, 403, 409} and not unqualified.exists(), {"httpStatus": status})


def blocked_writers(supervisor, thread_id, path):
    supervisor.api(f"/api/threads/{thread_id}/release", {})
    ready_file = Path(path) / "blocked-stdin-ready.json"
    ready_file.unlink(missing_ok=True)
    supervisor.prompt(thread_id, {"case": "blocked-stdin-writers"}, wait=False)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            ready = json.loads(ready_file.read_text())
            if ready.get("stdinBlocked") and ready.get("fixtureWillNotClean") and len(ready.get("pids", [])) == 2:
                return ready
        except (OSError, ValueError):
            pass
        time.sleep(0.1)
    raise Failure("Uncooperative blocked-stdin fixture did not start")


def writer_progress(path, pids):
    return [(Path(path) / ("detached-progress-" + str(pid) + ".txt")).read_text() for pid in pids]


def check_uncooperative_lifecycle(supervisor, thread_id, environment, connection, path, evidence, crash=False, host_canary_path=None):
    ready = blocked_writers(supervisor, thread_id, path)
    old_container = environment["containerId"]
    old_generation = environment["generation"]
    before = writer_progress(path, ready["pids"])
    time.sleep(0.3)
    require(before != writer_progress(path, ready["pids"]), "Uncooperative writers were not live before lifecycle fault")
    started = time.monotonic()
    if crash:
        supervisor.kill_for_recovery()
        supervisor.start()
        observed = next(item for item in supervisor.api("/api/environments")["environments"] if item["id"] == environment["id"])
    else:
        observed = supervisor.api(f"/api/environments/{environment['id']}/stop", {})
    elapsed = time.monotonic() - started
    prefix = "supervisor-sigkill-recovery" if crash else "stop-with-blocked-agent-stdin"
    old = docker_command(connection, "inspect", old_container, checked=False)
    stopped = old.returncode == 0 and json.loads(old.stdout)[0]["State"]["Running"] is False
    require(evidence.check(prefix + "-old-owned-container-stopped", stopped and elapsed < 45,
        {"elapsedSeconds": round(elapsed, 2), "state": observed["state"]}), "Uncooperative writer container was not quiesced")
    progress = writer_progress(path, ready["pids"])
    time.sleep(0.5)
    require(evidence.check(prefix + "-detached-writers-stopped", progress == writer_progress(path, ready["pids"])),
        "Old native writer survived lifecycle fencing")
    evidence.check(prefix + "-old-generation-fenced", observed["generation"] > old_generation and observed["state"] != "running")
    thread = supervisor.api(f"/api/threads/{thread_id}")["thread"]
    write_json(evidence.root / (prefix + "-thread-before-explicit-interrupt.json"),
        {"threadId": thread_id, "state": thread.get("status"), "environmentState": observed["state"]})
    # Recovery intentionally preserves an unfinished native turn as recovering.
    # Explicitly stop that old turn before issuing our new read-only request.
    supervisor.api(f"/api/threads/{thread_id}/interrupt", {})
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        thread = supervisor.api(f"/api/threads/{thread_id}")["thread"]
        if thread.get("status") not in {"running", "recovering"}:
            break
        time.sleep(0.1)
    require(thread.get("status") not in {"running", "recovering"}, "Old fixture turn did not settle after explicit interruption")
    if host_canary_path:
        check_web_file_routes(supervisor, thread_id, path, evidence, host_canary_path, prefix, running=False)
    replay_marker = (Path(path) / "blocked-stdin-ready.json").read_bytes()
    fresh = supervisor.api(f"/api/environments/{environment['id']}/start", {})
    require(fresh.get("state") == "running", "Environment did not explicitly restart after quiescence")
    time.sleep(1)
    evidence.check(prefix + "-blocked-prompt-not-replayed", replay_marker == (Path(path) / "blocked-stdin-ready.json").read_bytes())
    result = supervisor.prompt(thread_id, {"case": "stale-capability"})
    evidence.check(prefix + "-old-capability-denied-new-capability-works", result.get("staleCapabilityDenied") and result.get("currentCapabilityWorks"), result)
    environment.clear()
    environment.update(fresh)


class LegacyHostExecution:
    """Real host PTY and blocking command admission at the binding boundary."""
    def __init__(self, supervisor, thread_id, environment_id, root, host_canary, project_path, evidence):
        self.supervisor, self.thread_id, self.environment_id = supervisor, thread_id, environment_id
        self.evidence, self.host_canary = evidence, Path(host_canary)
        self.project_path, self.old_child = Path(project_path), None
        self.canary_before = (self.host_canary.read_bytes(), self.host_canary.stat().st_mtime_ns)
        self.root = root / ("host-binding-fence-" + uuid.uuid4().hex)
        self.root.mkdir()
        self.websocket, self.shell_id, self.viewer = None, None, None
        self.command_thread, self.command_result, self.command_error = None, None, None
        self.progress_at_ack = None

    def before_bind(self):
        api = self.supervisor.api
        shell = api(f"/api/threads/{self.thread_id}/shell", {"cols": 80, "rows": 24, "label": "Legacy host binding fixture"})
        self.shell_id = shell["activeShellId"]
        self.websocket = WebSocket(self.supervisor.base.replace("http://", "ws://") + "/ws",
            {"Authorization": "Bearer " + api.bearer})
        self.websocket.send_json({"type": "shell.attach", "shellId": self.shell_id, "cols": 80, "rows": 24})
        for _ in range(40):
            message = json.loads(self.websocket.receive())
            if message.get("type") == "shell.connected" and message.get("shellId") == self.shell_id:
                self.viewer = message["payload"]["viewerId"]
                break
        require(self.viewer, "Legacy host PTY did not attach")
        writer = ("import json,os,time;from pathlib import Path;r=Path(" + repr(str(self.root)) + ");"
            "r.joinpath('pty-pid.json').write_text(json.dumps({'pid':os.getpid(),'startTicks':Path('/proc/self/stat').read_text().split()[21]}));"
            "n=0\nwhile True:\n n+=1;r.joinpath('pty-progress.txt').write_text(str(n));time.sleep(.05)\n")
        self.websocket.send_json({"type": "shell.input", "shellId": self.shell_id, "viewerId": self.viewer,
            "data": "exec python3 -c " + shlex.quote(writer) + "\n"})
        progress = self.root / "pty-progress.txt"
        deadline = time.monotonic() + 6
        while not progress.exists() and time.monotonic() < deadline:
            time.sleep(.05)
        require(progress.exists(), "Legacy host PTY writer did not start")
        before = progress.read_bytes()
        time.sleep(.2)
        self.evidence.check("legacy-host-pty-writes-before-environment-bind", progress.read_bytes() != before)

        child = self.supervisor.prompt(self.thread_id, {"case": "child-create"})
        require(child.get("created") and child.get("childThreadId"), "Legacy host CLI child was not created")
        self.old_child = child["childThreadId"]
        origin = api(f"/api/threads/{self.old_child}/environment")
        self.evidence.check("legacy-child-starts-with-host-execution", origin.get("environment") is None)
        self.supervisor.prompt(self.old_child, {"case": "once"})
        self.child_counter_before = (self.project_path / "side-effect-count.txt").read_bytes()
        status, _ = api.request(f"/api/threads/{self.thread_id}/environment",
            {"environmentId": self.environment_id, "resetSession": True}, "PUT", allowed=(200, 400, 409))
        self.evidence.check("open-legacy-child-prevents-parent-environment-bind", status == 409,
            {"httpStatus": status, "childThreadId": self.old_child})
        api("/api/cli", {"operation": "close", "threadId": self.old_child, "fromThreadId": self.thread_id})
        closed = api(f"/api/threads/{self.old_child}")["thread"]
        require(self.evidence.check("legacy-child-explicitly-closed-before-parent-bind", bool(closed.get("closedAt"))), "Legacy child did not close")

        command = ("import time;from pathlib import Path;r=Path(" + repr(str(self.root)) + ");"
            "r.joinpath('command-ready').write_text('ready')\n"
            "while not r.joinpath('command-release').exists():time.sleep(.05)\n"
            "p=r/'command-count.txt';p.write_text(str(int(p.read_text())+1) if p.exists() else '1')\n")
        client = API(self.supervisor.base, self.evidence)
        client.bearer, client.cli_bearer = api.bearer, api.cli_bearer

        def run_command():
            try:
                self.command_result = client("/api/cli", {"operation": "commandRun", "threadId": self.thread_id,
                    "input": {"argv": ["python3", "-c", command], "cwd": ".", "timeoutSeconds": 25}})
            except Exception as error:
                self.command_error = type(error).__name__

        self.command_thread = threading.Thread(target=run_command, daemon=True)
        self.command_thread.start()
        try:
            deadline = time.monotonic() + 6
            while not (self.root / "command-ready").exists() and time.monotonic() < deadline:
                time.sleep(.05)
            require((self.root / "command-ready").exists(), "Real host command did not reach its blocking point")
            status, _ = api.request(f"/api/threads/{self.thread_id}/environment",
                {"environmentId": self.environment_id, "resetSession": True}, "PUT", allowed=(200, 400, 409))
            binding = api(f"/api/threads/{self.thread_id}/environment")
            self.evidence.check("running-host-command-prevents-environment-bind", status == 409 and binding.get("environment") is None,
                {"httpStatus": status, "bindingRemainsHost": binding.get("environment") is None})
        finally:
            (self.root / "command-release").write_text("release")
            self.command_thread.join(timeout=8)
        result = self.command_result or {}
        self.evidence.check("host-command-completes-once-before-explicit-bind", not self.command_thread.is_alive()
            and result.get("state") == "completed" and result.get("exitCode") == 0
            and (self.root / "command-count.txt").read_text() == "1",
            {"commandState": result.get("state"), "exitCode": result.get("exitCode"), "failureType": self.command_error})

    def capture_bind_ack(self):
        self.progress_at_ack = (self.root / "pty-progress.txt").read_bytes()

    def after_bind(self):
        forbidden = self.root / "old-pty-must-not-write"
        self.websocket.send_json({"type": "shell.input", "shellId": self.shell_id, "viewerId": self.viewer,
            "data": "python3 -c " + shlex.quote("from pathlib import Path;Path(" + repr(str(forbidden)) + ").write_text('unexpected')") + "\n"})
        self.websocket.socket.settimeout(.5)
        denied = False
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                message = json.loads(self.websocket.receive())
                if message.get("type") == "shell.error" and message.get("shellId") == self.shell_id:
                    denied = True
                    break
            except socket.timeout:
                continue
            except (EOFError, ConnectionError):
                break
        time.sleep(.5)
        self.evidence.check("legacy-host-pty-old-input-rejected-after-bind", denied and not forbidden.exists(),
            {"explicitShellError": denied, "hostWriteAbsent": not forbidden.exists()})
        pid = json.loads((self.root / "pty-pid.json").read_text())["pid"]
        self.evidence.check("legacy-host-pty-writer-stopped-at-bind-ack",
            (self.root / "pty-progress.txt").read_bytes() == self.progress_at_ack and not Path("/proc", str(pid)).exists())
        self.evidence.check("legacy-host-pty-bind-does-not-write-host-canary",
            self.canary_before == (self.host_canary.read_bytes(), self.host_canary.stat().st_mtime_ns))
        self.evidence.check("binding-does-not-replay-completed-host-command", (self.root / "command-count.txt").read_text() == "1")
        api = self.supervisor.api
        status, _ = api.request(f"/api/threads/{self.old_child}/environment", allowed=(200, 400, 403, 409))
        self.evidence.check("legacy-closed-child-environment-refuses-host-fallback", 400 <= status < 500, {"httpStatus": status})
        status, _ = api.request(f"/api/threads/{self.old_child}/prompt", {"prompt": "POCKY_ENV_CASE "
            + json.dumps({"case": "once", "caseId": uuid.uuid4().hex}), "clientRequestId": uuid.uuid4().hex}, allowed=(200, 400, 403, 409))
        time.sleep(.5)
        self.evidence.check("legacy-closed-child-prompt-cannot-reopen-on-host", 400 <= status < 500
            and (self.project_path / "side-effect-count.txt").read_bytes() == self.child_counter_before,
            {"httpStatus": status, "hostSideEffectUnchanged": (self.project_path / "side-effect-count.txt").read_bytes() == self.child_counter_before})
        forbidden_child = self.root / "old-child-command-must-not-write"
        status, result = api.request("/api/cli", {"operation": "commandRun", "threadId": self.old_child,
            "input": {"argv": ["python3", "-c", "from pathlib import Path;Path(" + repr(str(forbidden_child)) + ").write_text('unexpected')"],
                "cwd": ".", "timeoutSeconds": 5}}, allowed=(200, 400, 403, 409))
        self.evidence.check("legacy-closed-child-command-refused-without-host-write", 400 <= status < 500 and not forbidden_child.exists(),
            {"httpStatus": status, "hostWriteAbsent": not forbidden_child.exists()})

    def close(self):
        (self.root / "command-release").write_text("release")
        if self.command_thread:
            self.command_thread.join(timeout=3)
        if self.websocket:
            self.websocket.close()
        if self.shell_id:
            self.supervisor.api.request(f"/api/shells/{self.shell_id}/terminate", {}, allowed=(200, 400, 404))
        identity = self.root / "pty-pid.json"
        if identity.exists():
            saved = json.loads(identity.read_text())
            proc = Path("/proc", str(saved["pid"]))
            try:
                with os.fdopen(os.open(proc / "cmdline", os.O_RDONLY), "rb") as file:
                    matches = str(self.root).encode() in file.read()
                if matches and (proc / "stat").read_text().split()[21] == saved["startTicks"]:
                    descriptor = os.pidfd_open(saved["pid"])
                    try:
                        if (proc / "stat").read_text().split()[21] == saved["startTicks"]:
                            signal.pidfd_send_signal(descriptor, signal.SIGKILL)
                    finally:
                        os.close(descriptor)
            except (FileNotFoundError, ProcessLookupError):
                pass


def runtime_checks(supervisor, state, root, connection, evidence, model_upstream, relay, resume=False, pressure_gate=None):
    api = supervisor.api
    request = dict(connection, name=state["prefix"], image=state["image"],
        allowedRoots=[state["originalPath"], state["linkedPath"], str(root / "bin")],
        cpuLimit=0.25, memoryLimitBytes=268435456, pidsLimit=96, nofileLimit=1024)
    saved_thread_ids = list(state.get("threadIds", [])) if resume else []
    saved_environment_ids = list(state.get("environmentIds", [])) if resume else []
    requested_connection = state.get("connectionId", "new") if resume else "new"
    configured = api("/api/environments/connections/" + requested_connection, request, "PUT")
    evidence.check("connection-api-key-write-only", connection["apiKey"] not in json.dumps(configured) and "apiKey" not in configured)
    connection_id = configured["id"]
    state["connectionId"] = connection_id
    tested = api(f"/api/environments/connections/{connection_id}/test", {})
    require(evidence.check("real-dokploy-connectivity", tested.get("ok") is True), "Real Dokploy connection failed")
    api("/api/environments/budget", {"maxRunning": 2, "maxCpu": 1, "maxMemoryBytes": 805306368,
        "reservedCpu": 0, "reservedMemoryBytes": 0}, "PUT")
    workspaces, threads, environments = [], [], []
    for index, (label, path) in enumerate([("original", state["originalPath"]), ("linked", state["linkedPath"])]):
        (Path(path) / "host-updated.txt").write_text("host-fixture-update\n")
        if index < len(saved_thread_ids):
            snapshot = api("/api/threads/" + saved_thread_ids[index])
            thread = snapshot.get("thread", snapshot)
            if thread.get("status") in {"running", "recovering"}:
                api(f"/api/threads/{thread['id']}/interrupt", {})
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    thread = api(f"/api/threads/{thread['id']}")["thread"]
                    if thread.get("status") not in {"running", "recovering"}:
                        break
                    time.sleep(0.1)
                require(thread.get("status") not in {"running", "recovering"}, "Retained fixture turn could not be explicitly interrupted before resume")
            workspace = api("/api/workspaces/" + thread["workspaceId"])
            environment = next(item for item in api("/api/environments")["environments"] if item["id"] == saved_environment_ids[index])
            require(environment["name"].startswith(state["prefix"]) and environment["workspacePath"] == path, "Resume resource ownership differs")
        else:
            workspace = api("/api/workspaces", {"absPath": path, "label": state["prefix"] + "-" + label})
            thread = api("/api/threads/start", {"workspaceId": workspace["id"], "provider": "acp", "agentId": "custom",
                "model": "fixture", "title": state["prefix"] + "-" + label, "approvalMode": "yolo"})
            environment = api("/api/environments", {"connectionId": connection_id, "threadId": thread["id"], "name": state["prefix"] + "-" + label})
        workspaces.append(workspace)
        threads.append(thread)
        environments.append(environment)
        state.update({"connectionId": connection_id, "threadIds": [t["id"] for t in threads] + saved_thread_ids[len(threads):],
            "environmentIds": [e["id"] for e in environments] + saved_environment_ids[len(environments):]})
        write_json(root / "manifest.json", state)
        environment = api(f"/api/environments/{environment['id']}/start", {})
        require(evidence.check(label + "-environment-running", environment.get("state") == "running"), "Environment did not start")
        if label == "original":
            relay.legacy_host_binding(supervisor, thread["id"], environment["id"],
                LegacyHostExecution(supervisor, thread["id"], environment["id"], root, state["hostCanaryPath"], path, evidence))
        else:
            api(f"/api/threads/{thread['id']}/environment", {"environmentId": environment["id"], "resetSession": True}, "PUT")
        environments[-1] = environment
        saved_details = {e["id"]: e for e in state.get("environments", [])}
        saved_details.update({e["id"]: {k: e.get(k) for k in ["id", "threadId", "composeId", "containerId", "generation"]} for e in environments})
        state["environments"] = [saved_details[env_id] for env_id in state["environmentIds"] if env_id in saved_details]
        write_json(root / "manifest.json", state)
        inspected = json.loads(docker_command(connection, "inspect", environment["containerId"]).stdout)[0]
        host_config = inspected["HostConfig"]
        limits = {"nanoCpus": host_config["NanoCpus"], "memoryBytes": host_config["Memory"], "pidsLimit": host_config["PidsLimit"],
            "ulimits": host_config.get("Ulimits"), "networkMode": host_config["NetworkMode"], "privileged": host_config["Privileged"],
            "readonlyRootfs": host_config["ReadonlyRootfs"], "capDrop": host_config["CapDrop"], "user": inspected["Config"]["User"],
            "securityOpt": host_config.get("SecurityOpt", [])}
        evidence.check(label + "-actual-provider-security-and-limits", limits["nanoCpus"] == 250000000
            and limits["memoryBytes"] == 268435456 and limits["pidsLimit"] == 96 and limits["networkMode"] == "none"
            and not limits["privileged"] and limits["readonlyRootfs"] and "ALL" in limits["capDrop"]
            and limits["user"] == f"{os.getuid()}:{os.getgid()}" and "no-new-privileges:true" in limits["securityOpt"]
            and any(u["Name"] == "nofile" and u["Soft"] == 1024 and u["Hard"] == 1024 for u in (limits["ulimits"] or [])), limits)
        environment_values = "\n".join(inspected["Config"].get("Env", []))
        evidence.check(label + "-management-key-and-host-canary-not-in-container-config", connection["apiKey"] not in environment_values
            and (root / "private/canary").read_text() not in environment_values)
        identity = supervisor.prompt(thread["id"], {"case": "identity", "databasePath": state["databasePath"], "hostConfigPath": str(root / "home/.codex"),
            "daemonSocketPath": connection["dockerHost"].removeprefix("unix://")})
        require(evidence.check(label + "-identity-isolated", identity.get("workerMarked") and identity.get("cliSelfOk")
            and identity.get("uid") == os.getuid() and identity.get("uid") != 0
            and not identity.get("hostCanaryInEnv") and not identity.get("dockerSocketVisible") and not identity.get("dockerContextInEnv")
            and not identity.get("supervisorDatabaseVisible") and not identity.get("hostConfigVisible"), identity),
            "Isolation identity was not confirmed; refusing native/callback probes")
        host_stat = Path(path).stat()
        evidence.check(label + "-same-host-storage-inode", identity.get("cwdDevice") == host_stat.st_dev and identity.get("cwdInode") == host_stat.st_ino)
        if label == "original":
            if resume and state.get("childThreadId"):
                child_id = state["childThreadId"]
                child_snapshot = api(f"/api/threads/{child_id}")["thread"]
                require(child_snapshot.get("parentThreadId") == thread["id"], "Resumed child lineage differs")
                evidence.check("original-native-cli-child-created", True, {"childThreadId": child_id, "retainedFromEarlierAttempt": True})
            else:
                child = supervisor.prompt(thread["id"], {"case": "child-create"})
                require(evidence.check("original-native-cli-child-created", child.get("created") is True, child), "Original thread CLI child creation failed")
                child_id = child["childThreadId"]
                state["childThreadId"] = child_id
                write_json(root / "manifest.json", state)
            child_environment = api(f"/api/threads/{child_id}/environment")
            evidence.check("native-child-inherits-original-environment", child_environment.get("environment", {}).get("id") == environment["id"])
            try:
                child_identity = supervisor.prompt(child_id, {"case": "identity", "databasePath": state["databasePath"], "hostConfigPath": str(root / "home/.codex")})
                if resume and state.get("childThreadId"):
                    evidence.check("native-child-session-resume-after-supervisor-restart", True)
            except Failure as error:
                # Keep the resumed-session failure visible while a fresh child
                # lets independent original/linked checks continue.
                evidence.check("native-child-session-resume-after-supervisor-restart", False,
                    {"childThreadId": child_id, "failure": str(error)})
                child = supervisor.prompt(thread["id"], {"case": "child-create"})
                require(child.get("created"), "Fresh fixture child creation also failed")
                child_id = child["childThreadId"]
                state.setdefault("additionalChildThreadIds", []).append(child_id)
                write_json(root / "manifest.json", state)
                child_identity = supervisor.prompt(child_id, {"case": "identity", "databasePath": state["databasePath"], "hostConfigPath": str(root / "home/.codex")})
            evidence.check("native-child-execution-isolated", child_identity.get("workerMarked") and child_identity.get("cliSelfOk")
                and not child_identity.get("hostCanaryInEnv") and not child_identity.get("supervisorDatabaseVisible"), child_identity)
        for case, fields in [("native", ["nativeSubprocessOk", "hostEditObserved"]), ("fs", ["writeOk", "readMatches"]),
                ("terminal", ["created", "completed", "workerMarked"]), ("git", ["added", "committed", "statusOk", "stagedDiffObserved"])]:
            result = supervisor.prompt(thread["id"], {"case": case})
            evidence.check(label + "-" + case, all(result.get(field) is True for field in fields)
                and not result.get("hostCanaryInEnv", False), result)
        result = supervisor.prompt(thread["id"], {"case": "service", "port": 18765, "label": label})
        evidence.check(label + "-service", result.get("serviceReady") is True, result)
        result = supervisor.prompt(thread["id"], {"case": "model"})
        evidence.check(label + "-host-broker-model-sse", all(result.get(field) is True for field in
            ["requestOk", "sse", "deltaReceived", "completed", "incrementalStream"]), result)
        before_requests = len(model_upstream.requests)
        result = supervisor.prompt(thread["id"], {"case": "model-deny"})
        evidence.check(label + "-broker-profile-and-endpoint-denied-before-forwarding",
            result.get("otherHarnessProfileDenied") and result.get("wrongEndpointDenied") and len(model_upstream.requests) == before_requests, result)
        result = supervisor.prompt(thread["id"], {"case": "deny", "hostPath": state["hostCanaryPath"]})
        evidence.check(label + "-host-path-callbacks-denied", all(result.get(field) is True for field in
            ["readDenied", "writeDenied", "nativeReadDenied", "nativeWriteDenied", "symlinkReadDenied"])
            and not result.get("nativePathVisible"), result)
        result = supervisor.prompt(thread["id"], {"case": "gateway-deny"})
        evidence.check(label + "-forged-cli-and-host-mcp-denied", all(result.get(field) is True for field in
            ["forgedThreadDenied", "invalidCapabilityDenied", "hostMcpMutationDenied"]), result)
        result = supervisor.prompt(thread["id"], {"case": "network", "hostPort": state["port"]})
        evidence.check(label + "-native-and-proxy-network-denials", all(result.get(field) is True for field in
            ["nativeHostLoopbackDenied", "nativeMetadataDenied", "nativePublicRouteDenied", "proxyAvailable",
             "proxyUnauthorizedHostDenied", "proxyLiteralPrivateDenied", "proxyMetadataDenied"]), result)
        result = supervisor.prompt(thread["id"], {"case": "worktree", "create": True})
        evidence.check(label + "-managed-worktree-create-and-observation", result.get("listOk") and result.get("currentOk") and result.get("createOk"), result)
        output = supervisor.command(thread["id"], ["python3", "-c", "import os,json;print(json.dumps({'workerMarked':os.environ.get('POCKYMOE_ENVIRONMENT_WORKER')=='1'}))"])
        try:
            command_output = json.loads(output.get("stdout", ""))
        except ValueError:
            command_output = {}
        evidence.check(label + "-explicit-command-inside", output.get("exitCode") == 0 and command_output.get("workerMarked") is True)
        tree = api(f"/api/workspaces/{workspace['id']}/files/tree?threadId={thread['id']}&path=.")
        evidence.check(label + "-web-file-tree", isinstance(tree, dict) and tree.get("kind") == "directory")
        check_web_file_routes(supervisor, thread["id"], path, evidence, state["hostCanaryPath"], label, running=True)
        # The true same-port origin probe uses a worker TCP stream; full Relay
        # authorization, sharing and public preview tests remain the primary's job.
        container = environment.get("containerId")
        origin = docker_command(connection, "exec", container, "python3", "-c",
            "import urllib.request,json;print(json.load(urllib.request.urlopen('http://127.0.0.1:18765/'))['label'])").stdout.decode().strip()
        evidence.check(label + "-same-port-origin-isolated", origin == label)
        with WorkerForwarder(connection, container, 18765) as port:
            # Preserve independent evidence when one protocol fails; every
            # failure remains a failing acceptance check in the final result.
            for protocol in ["http", "sse", "websocket"]:
                try:
                    if protocol == "websocket":
                        with WebSocket(f"ws://127.0.0.1:{port}/ws") as websocket:
                            websocket.send("acceptance")
                            passed = websocket.receive() == label + ":acceptance"
                    else:
                        with urllib.request.urlopen(f"http://127.0.0.1:{port}/" + ("sse" if protocol == "sse" else ""), timeout=8) as reply:
                            passed = reply.read().count(b"event: fixture") == 3 if protocol == "sse" else json.load(reply).get("label") == label
                    evidence.check(label + "-rust-worker-" + protocol, passed)
                except (OSError, EOFError, ConnectionError) as error:
                    evidence.check(label + "-rust-worker-" + protocol, False, {"failureType": type(error).__name__})
        for encoded in [False, True]:
            check_worker_pty(supervisor, thread["id"], path, container, connection, evidence, state["hostCanaryPath"], label, encoded)

    relay.checks(supervisor, threads)
    check_host_mcp_rejected(supervisor, threads[0]["id"], root, evidence)
    evidence.check("linked-git-pointer-preserved", sha256_file(Path(state["linkedPath"]) / ".git") == state["linkedGitPointerSha256"])
    evidence.check("original-and-linked-agent-writes-real-host", all((Path(p) / "agent-change.txt").exists() for p in [state["originalPath"], state["linkedPath"]]))
    evidence.check("host-canary-content-preserved", (root / "private/canary").read_bytes() == Path(state["hostCanaryPath"]).read_bytes())
    evidence.check("tracked-source-ownership-and-inode-preserved", all(
        {"uid": (Path(path) / "tracked.txt").stat().st_uid, "gid": (Path(path) / "tracked.txt").stat().st_gid,
         "mode": (Path(path) / "tracked.txt").stat().st_mode, "inode": (Path(path) / "tracked.txt").stat().st_ino} == expected
        for path, expected in state["sourceBaselineStats"].items()))
    model_upstream.revision = 1
    model_upstream.activate(api)
    result = supervisor.prompt(threads[0]["id"], {"case": "model"})
    evidence.check("broker-host-key-rotation-without-container-copy", all(result.get(field) is True for field in
        ["requestOk", "sse", "deltaReceived", "completed", "incrementalStream"]) and any(request["revision"] == 1 and request["accepted"] for request in model_upstream.requests), result)
    write_json(root / "model-upstream-evidence.json", model_upstream.requests)
    model_secret_checks(root, state, connection, environments, api, model_upstream, evidence)

    # Real cancellation with a native descendant, then a finite side effect.
    tid, environment = threads[0]["id"], environments[0]
    if pressure_gate:
        pressure_gate = pressure_gate.resolve()
        require(pressure_gate.is_relative_to(root), "Pressure coordination marker must remain inside this run")
        write_json(root / "pressure-pending.json", {"gateFile": str(pressure_gate), "status": "waiting-for-shared-lab-build-to-finish"})
        print(json.dumps({"status": "pressure-waiting", "gateFile": str(pressure_gate)}), flush=True)
        deadline = time.monotonic() + 600
        while not pressure_gate.exists() and time.monotonic() < deadline:
            time.sleep(0.5)
        require(pressure_gate.exists(), "Shared lab image build has not finished; pressure probes remain pending")
    for case, fields in [("cpu-pressure", ["cpuThrottled"]), ("pids-pressure", ["pidsLimited"]),
            ("oom-pressure", ["nativeChildOomKilled", "oomCounterIncreased"])]:
        result = supervisor.prompt(tid, {"case": case})
        evidence.check("actual-provider-" + case, all(result.get(field) is True for field in fields), result)
    supervisor.prompt(tid, {"case": "wait"}, wait=False)
    pid_file = Path(state["originalPath"]) / "cancel-native.pid"
    deadline = time.monotonic() + 15
    while not pid_file.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    require(pid_file.exists(), "Cancellable native subprocess did not start")
    native_pid = pid_file.read_text().strip()
    api(f"/api/threads/{tid}/interrupt", {})
    time.sleep(1)
    alive = docker_command(connection, "exec", environment["containerId"], "python3", "-c",
        "import pathlib,sys;print(pathlib.Path('/proc/'+sys.argv[1]).exists())", native_pid).stdout.strip()
    evidence.check("cancel-native-process-domain-cleaned", alive == b"False")
    check_detached_cleanup(supervisor, tid, environment, connection, state["originalPath"], evidence)
    check_detached_cleanup(supervisor, tid, environment, connection, state["originalPath"], evidence, transport=True)
    check_uncooperative_lifecycle(supervisor, tid, environment, connection, state["originalPath"], evidence, host_canary_path=state["hostCanaryPath"])
    check_uncooperative_lifecycle(supervisor, tid, environment, connection, state["originalPath"], evidence, crash=True, host_canary_path=state["hostCanaryPath"])
    supervisor.prompt(tid, {"case": "once"})
    remembered = supervisor.prompt(tid, {"case": "remember-capability"})
    evidence.check("short-lived-capability-retained-privately-for-revocation-test", remembered.get("rememberedPrivately") is True)
    before = (Path(state["originalPath"]) / "side-effect-count.txt").read_text()
    docker_command(connection, "kill", "--signal", "KILL", environment["containerId"])
    interrupted = None
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        snapshot = api("/api/environments")
        interrupted = next(item for item in snapshot["environments"] if item["id"] == environment["id"])
        if interrupted["state"] != "running":
            break
        time.sleep(0.2)
    evidence.check("actual-worker-crash-has-explicit-state", interrupted["state"] in {"interrupted", "error"}, {"state": interrupted["state"]})
    forbidden_side_effect = root / ("unauthorized/fallback-must-not-execute-" + uuid.uuid4().hex)
    require(not forbidden_side_effect.exists(), "Fresh fallback probe marker unexpectedly exists")
    status, refused = api.request("/api/cli", {"operation": "commandRun", "threadId": tid, "input": {"argv": ["python3", "-c",
        "from pathlib import Path;Path(" + repr(str(forbidden_side_effect)) + ").write_text('unexpected')"], "cwd": ".", "timeoutSeconds": 5}}, allowed=(200, 400, 409, 500, 503))
    # CommandRun persists spawn failures as HTTP-200 command records. Require
    # the specific fail-closed state, no exit/output, and an absent fresh marker.
    failed_before_spawn = (isinstance(refused, dict) and refused.get("state") == "failed"
        and refused.get("exitCode") is None and not refused.get("stdout") and not refused.get("stderr")
        and "environment_not_running" in str(refused.get("error", "")))
    evidence.check("stopped-worker-command-refused-without-host-fallback",
        (400 <= status < 500 or (status == 200 and failed_before_spawn)) and not forbidden_side_effect.exists(),
        {"httpStatus": status, "commandId": refused.get("id") if isinstance(refused, dict) else None,
         "commandState": refused.get("state") if isinstance(refused, dict) else None,
         "environmentNotRunning": failed_before_spawn, "exitCode": refused.get("exitCode") if isinstance(refused, dict) else None,
         "preexistingBefore": False, "existsAfter": forbidden_side_effect.exists(),
         "mtimeAfter": forbidden_side_effect.stat().st_mtime_ns if forbidden_side_effect.exists() else None,
         "freshHostMarkerAbsent": not forbidden_side_effect.exists()})
    api(f"/api/environments/{environment['id']}/stop", {})
    restarted = api(f"/api/environments/{environment['id']}/start", {})
    evidence.check("provider-stop-start", restarted.get("state") == "running")
    time.sleep(1.5)
    evidence.check("stop-start-no-side-effect-replay", (Path(state["originalPath"]) / "side-effect-count.txt").read_text() == before)
    rebuilt = api(f"/api/environments/{environment['id']}/rebuild", {})
    evidence.check("provider-rebuild-generation-changed", rebuilt.get("generation", 0) > environment.get("generation", 0))
    # Do not prompt after rebuild automatically: a new prompt would be an
    # explicit new side effect, and would conceal replay bugs.
    time.sleep(1.5)
    evidence.check("rebuild-no-side-effect-replay", (Path(state["originalPath"]) / "side-effect-count.txt").read_text() == before)
    if rebuilt.get("state") != "running":
        api(f"/api/environments/{environment['id']}/start", {})
    revoked = supervisor.prompt(tid, {"case": "stale-capability"})
    evidence.check("stale-generation-capability-rejected-current-renewed", revoked.get("staleCapabilityDenied") and revoked.get("currentCapabilityWorks"), revoked)
    state["environmentIds"] = [e["id"] for e in environments]
    state["sideEffectCountBeforeSupervisorRestart"] = before
    write_json(root / "manifest.json", state)
    return state


def run_acceptance(args):
    state, root = load_state(args.manifest)
    require(args.resume or not state.get("environmentIds"), "Retained workers exist; use --resume after a new ready build, or prepare a fresh run")
    if args.resume:
        require(state.get("status") in {"failed", "failed-checks"}, "Resume is only for a retained failed acceptance")
    if (root / "summary.json").exists():
        archive = root / ("attempts/run-" + str(time.time_ns()))
        archive.mkdir(parents=True)
        for name in ["manifest.json", "summary.json", "checks.json", "http-events.jsonl", "http-errors.jsonl", "supervisor.redacted.log", "relay.redacted.log"]:
            if (root / name).exists():
                shutil.copy2(root / name, archive / name)
        for name in ["http-events.jsonl", "http-errors.jsonl"]:
            (root / name).unlink(missing_ok=True)
    require(args.ready_commit, "The primary must explicitly provide a ready commit before Supervisor startup")
    commit = process(["git", "rev-parse", "--verify", args.ready_commit + "^{commit}"], cwd=REPOSITORY).stdout.decode().strip()
    require(process(["git", "merge-base", "--is-ancestor", commit, "HEAD"], cwd=REPOSITORY, checked=False).returncode == 0,
        "Ready commit must belong to this checkout's history")
    binary = check_binary(args, state, root)
    # Pin a copy so another thread's cargo build cannot overwrite an executable
    # that the running test Supervisor and real containers both use.
    source_digest = sha256_file(binary)
    pinned = root / ("bin/pockymoe-" + source_digest[:20])
    if binary != pinned.resolve():
        if pinned.exists():
            require(sha256_file(pinned) == source_digest, "Existing pinned binary identity differs")
        else:
            shutil.copy2(binary, pinned)
            require(sha256_file(pinned) == source_digest, "Build binary changed while making its private pinned copy")
    pinned.chmod(0o755)
    state.update({"readyCommit": commit, "binary": str(pinned), "binarySha256": sha256_file(pinned), "status": "running"})
    state.pop("failure", None)
    write_json(root / "manifest.json", state)
    connection = load_connection(Path(state["connectionFile"]))
    canary = (root / "private/canary").read_text()
    evidence = Evidence(root, [connection["apiKey"], canary])
    model_upstream = ModelUpstream(root, evidence)
    from relay_probe import Relay
    relay = Relay(root, pinned, evidence, API, basic_env, private_write, require)
    supervisor = None
    try:
        relay_config = relay.start()
        state.update({"relayBaseUrl": relay.base, "relayDeviceId": relay.device_id})
        supervisor = Supervisor(state, root, pinned, evidence, relay_config)
        for path in [Path(state["originalPath"]), Path(state["linkedPath"])]:
            for name in ["acp_agent.py", "fixture_service.py"]:
                shutil.copyfile(HERE / name, path / ".fixture" / name)
            if not (path / "escape-to-host").is_symlink():
                os.symlink(state["hostCanaryPath"], path / "escape-to-host")
        state.setdefault("sourceBaselineStats", {path: {"uid": (Path(path) / "tracked.txt").stat().st_uid,
            "gid": (Path(path) / "tracked.txt").stat().st_gid, "mode": (Path(path) / "tracked.txt").stat().st_mode,
            "inode": (Path(path) / "tracked.txt").stat().st_ino} for path in [state["originalPath"], state["linkedPath"]]})
        supervisor.start()
        relay.connected()
        model_upstream.activate(supervisor.api)
        runtime_checks(supervisor, state, root, connection, evidence, model_upstream, relay, args.resume, args.pressure_gate)
        # Restart only our Popen-owned Supervisor and retain its private DB.
        supervisor.stop()
        supervisor.start()
        recovered = supervisor.api("/api/environments")
        evidence.check("dedicated-supervisor-restart-state-recovered", all(any(env["id"] == env_id for env in recovered["environments"])
            for env_id in state["environmentIds"]))
        time.sleep(1.5)
        evidence.check("dedicated-supervisor-restart-no-completed-command-replay",
            (Path(state["originalPath"]) / "side-effect-count.txt").read_text() == state["sideEffectCountBeforeSupervisorRestart"])
        if all(check["passed"] for check in evidence.checks):
            for env_id in state["environmentIds"]:
                supervisor.api(f"/api/environments/{env_id}/destroy", {})
            evidence.check("destroy-preserves-original-linked-files-and-git", all((Path(p) / "agent-change.txt").exists()
                and len(git(Path(p), "rev-parse", "HEAD")) == 40 for p in [state["originalPath"], state["linkedPath"]]))
        state["status"] = "passed" if all(check["passed"] for check in evidence.checks) else "failed-checks"
    except Exception as error:
        state["status"] = "failed"
        state["failure"] = evidence.redact(str(error)) if isinstance(error, Failure) else type(error).__name__
        evidence.check("acceptance-execution", False, state["failure"])
    finally:
        if supervisor:
            supervisor.stop()
        relay.stop()
        model_upstream.close()
        raw_log = root / "private/supervisor.log"
        if raw_log.exists():
            evidence.check("model-canary-absent-from-supervisor-log", all(value not in raw_log.read_text(errors="replace") for value in model_upstream.secrets))
            (root / "supervisor.redacted.log").write_text(evidence.redact(raw_log.read_text(errors="replace")))
        evidence.audit_public_files()
        if state["status"] == "passed" and any(not check["passed"] for check in evidence.checks):
            state["status"] = "failed-checks"
        write_json(root / "manifest.json", state)
        write_json(SCRATCH / "current.json", state)
        write_json(root / "summary.json", {"status": state["status"], "readyCommit": commit, "binarySha256": state["binarySha256"],
            "checksPassed": sum(c["passed"] for c in evidence.checks), "checksFailed": sum(not c["passed"] for c in evidence.checks),
            "limitations": ["Deterministic ACP agent, real Rust transport and Dokploy provider; no real model turn",
                "Real isolated Relay HTTP/WS/SSE sharing/revocation uses private fixture accounts; no browser UI assertion",
                "Host-only fake model upstream validates real broker SSE and key rotation without a live model",
                "Dedicated stress command validates actual Docker CPU/OOM/PIDs separately"], "manifest": str(root / "manifest.json")})
    print(json.dumps({"status": state["status"], "summary": str(root / "summary.json"), "checks": str(root / "checks.json")}))
    return 0 if state["status"] == "passed" else 1


def stress(args):
    state, root = load_state(args.manifest)
    connection = load_connection(Path(state["connectionFile"]))
    stress_root = root / "stress"
    stress_root.mkdir(exist_ok=True)
    evidence = Evidence(stress_root, [connection["apiKey"]])
    api = API(connection["baseUrl"], evidence, connection["apiKey"])
    results = []
    for mode in ["cpu", "pids", "oom"]:
        destination = stress_root / mode
        destination.mkdir(exist_ok=True)
        compose_id = None
        try:
            specification = {"services": {"probe": {
                "image": state["image"], "entrypoint": ["python3", "/opt/pockymoe-test/stress_probe.py"], "command": [mode],
                "user": "1000:1000", "network_mode": "none", "read_only": True,
                "init": True, "cpus": 0.125, "mem_limit": "64m", "memswap_limit": "64m", "pids_limit": 32,
                "cap_drop": ["ALL"], "security_opt": ["no-new-privileges:true"],
                "labels": {"pockymoe.acceptance": state["prefix"]},
                "volumes": [{"type": "bind", "source": str(destination), "target": "/probe"}],
                "logging": {"driver": "json-file", "options": {"max-size": "1m", "max-file": "1"}},
            }}}
            service = api("/api/compose.create", {"name": state["prefix"] + "-stress-" + mode,
                "appName": state["prefix"] + "-" + mode, "environmentId": connection["dokployEnvironmentId"],
                "sourceType": "raw", "composeType": "docker-compose"})
            compose_id, app_name = service["composeId"], service["appName"]
            write_json(destination / "resource.json", {"composeId": compose_id, "appName": app_name})
            api("/api/compose.update", {"composeId": compose_id, "composeFile": json.dumps(specification),
                "isolatedDeployment": False, "isolatedDeploymentsVolume": True,
                "serviceNetworks": [{"serviceName": "probe", "networkIds": [], "detachDokployNetwork": True}]})
            api("/api/compose.deploy", {"composeId": compose_id, "title": "Bounded disposable " + mode + " cgroup probe"})
            deployment = None
            for _ in range(40):
                deployments = api("/api/deployment.allByCompose?composeId=" + compose_id)
                deployment = deployments[0] if deployments else None
                if deployment and deployment.get("status") in {"done", "error"}:
                    break
                time.sleep(0.25)
            require(deployment and deployment.get("status") == "done", "Real stress Compose deployment failed: " + mode)
            container_id = None
            detail = None
            for _ in range(60):
                ids = docker_command(connection, "ps", "-aq", "--filter", "label=com.docker.compose.project=" + app_name).stdout.decode().split()
                if ids:
                    container_id = ids[0]
                    detail = json.loads(docker_command(connection, "inspect", container_id).stdout)[0]
                    if not detail["State"]["Running"]:
                        break
                time.sleep(0.15)
            require(detail and not detail["State"]["Running"], "Bounded stress container did not terminate: " + mode)
            config = detail["HostConfig"]
            result = {"mode": mode, "composeId": compose_id, "deploymentId": deployment["deploymentId"],
                "containerId": container_id, "exitCode": detail["State"]["ExitCode"], "oomKilled": detail["State"]["OOMKilled"],
                "observedLimits": {"nanoCpus": config["NanoCpus"], "memoryBytes": config["Memory"],
                    "pidsLimit": config["PidsLimit"], "networkMode": config["NetworkMode"]}}
            require(config["NanoCpus"] == 125000000 and config["Memory"] == 67108864 and config["PidsLimit"] == 32
                and config["NetworkMode"] == "none", "Stress cgroup limits were not applied")
            if mode == "oom":
                passed = result["oomKilled"] and result["exitCode"] == 137
            else:
                require((destination / "result.json").exists(), "Stress probe evidence missing: " + mode)
                probe = json.loads((destination / "result.json").read_text())
                result["probe"] = probe
                passed = result["exitCode"] == 0 and (probe.get("nrThrottledDelta", 0) > 0 if mode == "cpu" else probe.get("pidsLimited") is True)
            result["passed"] = passed
            evidence.check("real-dokploy-" + mode, passed, result)
            results.append(result)
        except Failure as error:
            evidence.check("real-dokploy-" + mode, False, str(error))
        finally:
            if compose_id:
                # These are this command's fresh disposable probes, never lab
                # management services or primary-thread worker environments.
                api("/api/compose.delete", {"composeId": compose_id, "deleteVolumes": False})
    write_json(stress_root / "summary.json", {"passed": len(results) == 3 and all(r["passed"] for r in results), "results": results})
    print(json.dumps({"stressPassed": len(results) == 3 and all(r["passed"] for r in results), "evidence": str(stress_root / "summary.json")}))
    return 0 if len(results) == 3 and all(r["passed"] for r in results) else 1


def cleanup_workers(args):
    state, root = load_state(args.manifest)
    require(args.after_primary_done, "Keep workers for diagnosis until the primary is finished")
    binary = root / "bin/pockymoe"
    require(state.get("readyCommit") and binary.exists() and sha256_file(binary) == state.get("binarySha256"),
        "Cleanup may only restart this run's explicitly approved pinned test Supervisor")
    connection = load_connection(Path(state["connectionFile"]))
    values = [connection["apiKey"], (root / "private/canary").read_text()]
    values.extend(path.read_text() for path in (root / "private").glob("model-canary-*"))
    evidence = Evidence(root / "cleanup", values)
    evidence.root.mkdir(exist_ok=True)
    supervisor = Supervisor(state, root, binary, evidence)
    try:
        supervisor.start()
        environments = supervisor.api("/api/environments")["environments"]
        for environment in environments:
            require(environment["name"].startswith(state["prefix"]) and environment["workspacePath"] in
                {state["originalPath"], state["linkedPath"]}, "Refusing cleanup of an environment outside this run")
            if environment["state"] != "destroyed":
                destroyed = supervisor.api(f"/api/environments/{environment['id']}/destroy", {})
                require(destroyed["state"] == "destroyed", "Worker cleanup did not finish")
            evidence.check("destroyed-" + environment["id"], True)
        state["workersCleaned"] = True
        write_json(root / "manifest.json", state)
    finally:
        supervisor.stop()
        log = root / "private/supervisor.log"
        if log.exists():
            (root / "supervisor.redacted.log").write_text(evidence.redact(log.read_text(errors="replace")))
    print(json.dumps({"workersCleaned": True, "baseLabRetained": True, "evidence": str(evidence.root / "checks.json")}))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    prepare_parser = subparsers.add_parser("prepare", help="Build image and new real Git/linked-worktree fixtures; no Supervisor")
    prepare_parser.add_argument("--connection-file", type=Path, required=True)
    prepare_parser.add_argument("--image")
    prepare_parser.add_argument("--skip-build", action="store_true")
    binary_parser = subparsers.add_parser("check-binary", help="Run only container ldd; never start a Supervisor")
    binary_parser.add_argument("--manifest", type=Path)
    binary_parser.add_argument("--binary", type=Path, required=True)
    run_parser = subparsers.add_parser("run", help="Run the real API/provider acceptance after the primary's ready message")
    run_parser.add_argument("--manifest", type=Path)
    run_parser.add_argument("--binary", type=Path, required=True)
    run_parser.add_argument("--ready-commit", required=True)
    run_parser.add_argument("--resume", action="store_true", help="Resume this manifest's retained failed resources after a newly approved build")
    run_parser.add_argument("--pressure-gate", type=Path, help="Wait for this run-local coordination marker before pressure while another lab image builds")
    stress_parser = subparsers.add_parser("stress", help="Actual bounded CPU/PIDs/OOM probes via the lab's real Dokploy API; no Supervisor")
    stress_parser.add_argument("--manifest", type=Path)
    cleanup_parser = subparsers.add_parser("cleanup-workers", help="Destroy only this run's retained workers, leaving the base lab")
    cleanup_parser.add_argument("--manifest", type=Path)
    cleanup_parser.add_argument("--after-primary-done", action="store_true")
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args)
        return 0
    if args.command == "check-binary":
        check_binary(args)
        return 0
    if args.command == "stress":
        return stress(args)
    if args.command == "cleanup-workers":
        return cleanup_workers(args)
    return run_acceptance(args)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Failure as error:
        print(json.dumps({"status": "blocked", "reason": str(error)}))
        raise SystemExit(1)
