#!/usr/bin/env python3
"""Deterministic ACP fixture. No model credentials, prompt echo or env dumps."""
import json
import base64
import itertools
import os
import queue
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import urllib.parse
import uuid
from pathlib import Path

WRITE_LOCK = threading.Lock()
PENDING_LOCK = threading.Lock()
PENDING = {}
SESSIONS = {}
ACTIVE = {}
CHILDREN = []
MARKER = "POCKY_ENV_CASE "
REQUEST_IDS = itertools.count(900000)


def send(value):
    with WRITE_LOCK:
        sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
        sys.stdout.flush()


def response(request_id, value):
    send({"jsonrpc": "2.0", "id": request_id, "result": value})


def callback(method, parameters, cancel, timeout=12):
    # The runtime ACP transport currently supports numeric request IDs.
    request_id = next(REQUEST_IDS)
    channel = queue.Queue(maxsize=1)
    with PENDING_LOCK:
        PENDING[request_id] = channel
    send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": parameters})
    deadline = time.monotonic() + timeout
    try:
        while time.monotonic() < deadline:
            if cancel.is_set():
                raise InterruptedError
            try:
                message = channel.get(timeout=0.1)
                if "error" in message:
                    return {"denied": True, "responded": True}
                return {"denied": False, "responded": True, "value": message.get("result", {})}
            except queue.Empty:
                continue
        return {"denied": False, "responded": False}
    finally:
        with PENDING_LOCK:
            PENDING.pop(request_id, None)


def command(argv, cwd, timeout=20):
    result = subprocess.run(argv, cwd=cwd, capture_output=True, timeout=timeout)
    return result.returncode, result.stdout


def report(session_id, result):
    # Every result is explicitly assembled; no exception, arbitrary stdout,
    # credentials, environment values or request bodies can enter the transcript.
    send({"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": session_id, "update": {"sessionUpdate": "agent_message_chunk",
        "content": {"type": "text", "text": "POCKY_ENV_RESULT " + json.dumps(result, separators=(",", ":"))}}}})


def cli_json(argv, cwd, diagnostics=None):
    binary = "/opt/pockymoe/bin/pockymoe"
    # Local fixture self-check may opt into a specific built binary, never PATH.
    if os.environ.get("POCKY_ENV_FIXTURE_SELF_CHECK_BINARY"):
        binary = os.environ["POCKY_ENV_FIXTURE_SELF_CHECK_BINARY"]
    completed = subprocess.run([binary, *argv], cwd=cwd, capture_output=True, timeout=20)
    if diagnostics is not None:
        # Classify a fixed set of known failures without serializing stderr or
        # arbitrary response strings into the agent transcript.
        errors = completed.stderr.lower()
        diagnostics.update({"cliExitCode": completed.returncode,
            "modelUnavailable": b"model not available" in errors,
            "argumentRejected": b"unexpected argument" in errors or b"invalid value" in errors,
            "environmentRejected": b"environment" in errors,
            "permissionRejected": b"forbidden" in errors or b"unauthorized" in errors,
            "nameAlreadyUsed": b"name" in errors and (b"already" in errors or b"unique" in errors)})
    if completed.returncode:
        return None
    try:
        return json.loads(completed.stdout)
    except (ValueError, UnicodeDecodeError):
        return None


def gateway(body, token=None):
    base = os.environ.get("POCKYMOE_URL", "http://127.0.0.1:45680")
    req = urllib.request.Request(base + "/api/cli", data=json.dumps(body).encode(), headers={
        "Authorization": "Bearer " + (token or os.environ.get("POCKYMOE_TOKEN", "")),
        "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=8) as reply:
            reply.read(1024 * 1024)
            return reply.status
    except urllib.error.HTTPError as error:
        return error.code
    except (urllib.error.URLError, TimeoutError):
        return 0


def execute(case, session_id, cancel):
    cwd = Path(SESSIONS[session_id])
    name = case.get("case", "identity")
    result = {"case": name, "fixture": True}
    if case.get("caseId"):
        result["caseId"] = case["caseId"]
    if name == "identity":
        result.update({"uid": os.getuid(), "cwd": str(cwd), "workerMarked": os.environ.get("POCKYMOE_ENVIRONMENT_WORKER") == "1",
            "cwdDevice": cwd.stat().st_dev, "cwdInode": cwd.stat().st_ino,
            "hostCanaryInEnv": "POCKY_ENV_HOST_CANARY" in os.environ,
            "dockerSocketVisible": Path("/var/run/docker.sock").exists() or bool(case.get("daemonSocketPath")) and Path(case["daemonSocketPath"]).exists(),
            "dockerContextInEnv": any(name in os.environ for name in ["DOCKER_HOST", "DOCKER_CONTEXT"]),
            "supervisorDatabaseVisible": bool(case.get("databasePath")) and Path(case["databasePath"]).exists(),
            "hostConfigVisible": bool(case.get("hostConfigPath")) and Path(case["hostConfigPath"]).exists()})
        identity = cli_json(["thread", "self"], cwd)
        result["cliSelfOk"] = isinstance(identity, dict) and identity.get("threadId") == os.environ.get("POCKYMOE_THREAD_ID")
        if isinstance(identity, dict):
            result["threadId"] = identity.get("threadId")
    elif name == "native":
        path = cwd / "native.txt"
        code, _ = command([sys.executable, "-c", "from pathlib import Path; Path('native.txt').write_text('native-subprocess\\n')"], cwd)
        host_edit = cwd / "host-updated.txt"
        result.update({"nativeSubprocessOk": code == 0 and path.read_text() == "native-subprocess\n",
            "hostEditObserved": host_edit.exists() and host_edit.read_text() == "host-fixture-update\n"})
    elif name in {"detached-writers", "blocked-stdin-writers"}:
        if os.environ.get("POCKYMOE_ENVIRONMENT_WORKER") != "1":
            result["refusedOutsideWorker"] = True
        else:
            # Deliberately excluded from CHILDREN and all fixture cleanup.
            # Only the real Rust worker may reclaim these detached writers.
            code = """import json,os,signal,time
from pathlib import Path
os.setsid()
if os.fork():os._exit(0)
signal.signal(signal.SIGTERM,signal.SIG_IGN)
child=os.fork()
if child:Path('detached-pids.json').write_text(json.dumps([os.getpid(),child]))
while True:
 Path('detached-progress-'+str(os.getpid())+'.txt').write_text(str(time.monotonic()))
 time.sleep(0.1)
"""
            pid_file = cwd / "detached-pids.json"
            pid_file.unlink(missing_ok=True)
            launcher = subprocess.Popen([sys.executable, "-c", code], cwd=cwd, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            launcher.wait(timeout=4)
            deadline = time.monotonic() + 4
            while not pid_file.exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            pids = json.loads(pid_file.read_text())
            result.update({"started": len(pids) == 2, "pids": pids, "fixtureWillNotClean": True})
            if name == "blocked-stdin-writers":
                fd = os.open("/pockymoe/state/fixture-old-capability", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
                with os.fdopen(fd, "w") as file:
                    file.write(os.environ.get("POCKYMOE_TOKEN", ""))
                result["stdinBlocked"] = True
                (cwd / "blocked-stdin-ready.json").write_text(json.dumps(result))
                report(session_id, result)
                # Runs on the input thread; cancel messages cannot be read,
                # and neither this agent nor its writers cooperate with TERM.
                while True:
                    time.sleep(1)
    elif name == "child-create":
        child = cli_json(["thread", "create", "--provider", "acp", "--agent", "custom", "--model", "default",
            "--name", "fixture-native-child-" + uuid.uuid4().hex[:8], "--title", "Environment fixture child"], cwd, result)
        result["created"] = isinstance(child, dict) and isinstance(child.get("threadId"), str)
        if result["created"]:
            result["childThreadId"] = child["threadId"]
    elif name == "preview":
        mapping = cli_json(["preview", "create", "--port", str(case.get("port", 18765)), "--label", "Environment fixture preview"], cwd)
        result["created"] = isinstance(mapping, dict) and isinstance(mapping.get("id"), str)
        if result["created"]:
            result.update({key: mapping.get(key) for key in ["id", "threadId", "hostname"]})
    elif name == "preview-stop":
        stopped = cli_json(["preview", "stop", case["id"]], cwd)
        result["stopped"] = isinstance(stopped, dict) and stopped.get("stopped") is True
    elif name == "model-deny":
        base = os.environ.get("POCKYMOE_URL", "http://127.0.0.1:45680")
        for field, endpoint in [("otherHarnessProfileDenied", "/model/claude/v1/messages"),
                ("wrongEndpointDenied", "/model/codex/v1/messages")]:
            request = urllib.request.Request(base + endpoint, data=json.dumps({"model": "fixture", "messages": [], "stream": True}).encode(),
                headers={"Authorization": "Bearer " + os.environ.get("POCKYMOE_TOKEN", ""), "Content-Type": "application/json"})
            try:
                with urllib.request.urlopen(request, timeout=8) as reply:
                    reply.read(1048576)
                    result[field] = False
            except urllib.error.HTTPError as error:
                result[field] = error.code >= 400
            except OSError:
                result[field] = False
    elif name == "model":
        base = os.environ.get("POCKYMOE_URL", "http://127.0.0.1:45680")
        body = {"model": "fixture", "stream": True, "input": "environment acceptance"}
        request = urllib.request.Request(base + "/model/codex/responses", data=json.dumps(body).encode(), headers={
            "Authorization": "Bearer " + os.environ.get("POCKYMOE_TOKEN", ""), "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=15) as reply:
                first_line = reply.readline(4096)
                first_received = time.monotonic()
                streamed = first_line + reply.read(1048576)
                result.update({"requestOk": reply.status == 200, "sse": "text/event-stream" in reply.headers.get("Content-Type", ""),
                    "deltaReceived": b"environment-fixture-delta" in streamed, "completed": b"response.completed" in streamed,
                    "incrementalStream": first_line.startswith(b"event: ") and time.monotonic() - first_received >= 0.12})
        except (OSError, urllib.error.URLError):
            result["requestOk"] = False
    elif name == "cpu-pressure":
        def cpu_stat():
            return dict((key, int(value)) for key, value in (line.split() for line in Path("/sys/fs/cgroup/cpu.stat").read_text().splitlines()))
        before = cpu_stat()
        deadline = time.monotonic() + 1.5
        number = 1
        while time.monotonic() < deadline:
            number = (number * 1664525 + 1013904223) & 0xFFFFFFFF
        after = cpu_stat()
        result["cpuThrottled"] = after.get("nr_throttled", 0) > before.get("nr_throttled", 0)
        result["cpuMax"] = Path("/sys/fs/cgroup/cpu.max").read_text().strip()
    elif name == "pids-pressure":
        import errno
        children = []
        try:
            result["pidsLimited"] = False
            for _ in range(160):
                try:
                    pid = os.fork()
                except OSError as error:
                    result["pidsLimited"] = error.errno == errno.EAGAIN
                    break
                if pid == 0:
                    time.sleep(8)
                    os._exit(0)
                children.append(pid)
            result["childrenBeforeLimit"] = len(children)
            result["pidsMax"] = Path("/sys/fs/cgroup/pids.max").read_text().strip()
        finally:
            for pid in children:
                try:
                    os.kill(pid, signal.SIGTERM)
                    os.waitpid(pid, 0)
                except (OSError, ChildProcessError):
                    pass
    elif name == "oom-pressure":
        events = Path("/sys/fs/cgroup/memory.events")
        maximum = Path("/sys/fs/cgroup/memory.max").read_text().strip()
        if maximum != "268435456":
            result["refusedUnexpectedBudget"] = True
        else:
            before = dict(line.split() for line in events.read_text().splitlines())
            child = subprocess.run([sys.executable, "-c", "x=bytearray(512*1024*1024)"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
            after = dict(line.split() for line in events.read_text().splitlines())
            result.update({"nativeChildOomKilled": child.returncode in {-9, 137},
                "oomCounterIncreased": int(after.get("oom_kill", 0)) > int(before.get("oom_kill", 0))})
    elif name == "fs":
        path = str(cwd / "callback.txt")
        written = callback("fs/write_text_file", {"sessionId": session_id, "path": path, "content": "acp-callback\n"}, cancel)
        read = callback("fs/read_text_file", {"sessionId": session_id, "path": path}, cancel)
        result.update({"writeResponded": written["responded"], "writeOk": written["responded"] and not written["denied"],
            "readResponded": read["responded"], "readMatches": read.get("value", {}).get("content") == "acp-callback\n"})
    elif name == "terminal":
        created = callback("terminal/create", {"sessionId": session_id, "command": "/usr/bin/python3", "args": ["-c",
            "import json,os;from pathlib import Path;Path('terminal.txt').write_text('acp-terminal\\n');print(json.dumps({'workerMarked':os.environ.get('POCKYMOE_ENVIRONMENT_WORKER')=='1','hostCanaryInEnv':'POCKY_ENV_HOST_CANARY' in os.environ}))"],
            "cwd": str(cwd), "outputByteLimit": 65536}, cancel)
        terminal = created.get("value", {}).get("terminalId")
        result["created"] = bool(terminal)
        if terminal:
            waited = callback("terminal/wait_for_exit", {"sessionId": session_id, "terminalId": terminal}, cancel)
            output = callback("terminal/output", {"sessionId": session_id, "terminalId": terminal}, cancel)
            value = output.get("value", {}).get("output", "")
            try:
                payload = json.loads(value)
            except ValueError:
                payload = {}
            result.update({"completed": waited["responded"] and not waited["denied"],
                "workerMarked": payload.get("workerMarked") is True, "hostCanaryInEnv": payload.get("hostCanaryInEnv") is True})
            callback("terminal/release", {"sessionId": session_id, "terminalId": terminal}, cancel)
    elif name == "git":
        (cwd / "agent-change.txt").write_text("environment-fixture-change " + uuid.uuid4().hex + "\n")
        args = ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
                "-c", "user.name=Environment Fixture", "-c", "user.email=fixture@test.invalid"]
        added, _ = command(args + ["add", "agent-change.txt"], cwd)
        diff_code, diff = command(args + ["diff", "--cached", "--name-only"], cwd)
        committed, _ = command(args + ["commit", "-qm", "Environment fixture change"], cwd)
        status, output = command(args + ["status", "--porcelain"], cwd)
        head_code, head = command(args + ["rev-parse", "HEAD"], cwd)
        result.update({"added": added == 0, "committed": committed == 0, "statusOk": status == 0,
            "stagedDiffObserved": diff_code == 0 and b"agent-change.txt" in diff,
            "head": head.decode().strip() if head_code == 0 else None})
    elif name == "worktree":
        listed = cli_json(["worktree", "list"], cwd)
        current = cli_json(["worktree", "current"], cwd)
        result.update({"listOk": isinstance(listed, dict), "currentOk": isinstance(current, dict)})
        if case.get("create"):
            created = cli_json(["worktree", "create", "--name", "fixture-managed", "--request-id", "environment-fixture-managed"], cwd)
            result["createOk"] = isinstance(created, dict) and isinstance(created.get("workingCopy"), dict)
            if result["createOk"]:
                result["workingCopyPath"] = created["workingCopy"].get("path")
    elif name == "service":
        port = int(case.get("port", 18765))
        label = case.get("label", "fixture")
        child = subprocess.Popen([sys.executable, str(Path(__file__).with_name("fixture_service.py")), "--port", str(port), "--label", label],
            cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        CHILDREN.append(child)
        ready = False
        for _ in range(60):
            if cancel.is_set():
                raise InterruptedError
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=0.2) as reply:
                    ready = json.load(reply).get("label") == label
                if ready:
                    break
            except (OSError, ValueError):
                pass
            time.sleep(0.05)
        result.update({"serviceReady": ready, "port": port, "label": label})
    elif name == "deny":
        target = case["hostPath"]
        read = callback("fs/read_text_file", {"sessionId": session_id, "path": target}, cancel)
        write = callback("fs/write_text_file", {"sessionId": session_id, "path": target, "content": "blocked-write-attempt\n"}, cancel)
        result.update({"readResponded": read["responded"], "readDenied": read["denied"],
            "writeResponded": write["responded"], "writeDenied": write["denied"],
            "nativePathVisible": Path(target).exists()})
        try:
            Path(target).read_bytes()
            result["nativeReadDenied"] = False
        except OSError:
            result["nativeReadDenied"] = True
        try:
            Path(target).write_text("blocked-native-write-attempt\n")
            result["nativeWriteDenied"] = False
        except OSError:
            result["nativeWriteDenied"] = True
        symlink_read = callback("fs/read_text_file", {"sessionId": session_id, "path": str(cwd / "escape-to-host")}, cancel)
        result["symlinkReadDenied"] = symlink_read["responded"] and symlink_read["denied"]
    elif name == "gateway-deny":
        base_body = {"operation": "status", "threadId": os.environ.get("POCKYMOE_THREAD_ID")}
        result["forgedThreadDenied"] = gateway(dict(base_body, fromThreadId="00000000-0000-0000-0000-000000000000")) >= 400
        result["invalidCapabilityDenied"] = gateway(base_body, token="invalid-fixture-capability") >= 400
        result["hostMcpMutationDenied"] = gateway({"operation": "mcpSet", "name": "fixture-blocked", "command": "sh"}) >= 400
    elif name == "network":
        for field, address in [("nativeHostLoopbackDenied", ("127.0.0.1", int(case["hostPort"]))),
                ("nativeMetadataDenied", ("169.254.169.254", 80)), ("nativePublicRouteDenied", ("1.1.1.1", 443))]:
            try:
                with socket.create_connection(address, timeout=0.5):
                    result[field] = False
            except OSError:
                result[field] = True
        proxy = urllib.parse.urlsplit(os.environ.get("HTTPS_PROXY", ""))
        result["proxyAvailable"] = False
        for field, authority in [("proxyUnauthorizedHostDenied", "unapproved.test.invalid:443"),
                ("proxyLiteralPrivateDenied", "127.0.0.1:443"), ("proxyMetadataDenied", "169.254.169.254:443")]:
            result[field] = False
            try:
                with socket.create_connection((proxy.hostname, proxy.port), timeout=2) as stream:
                    result["proxyAvailable"] = True
                    authentication = base64.b64encode(((proxy.username or "") + ":" + (proxy.password or "")).encode()).decode()
                    stream.sendall(("CONNECT " + authority + " HTTP/1.1\r\nHost: " + authority + "\r\nProxy-Authorization: Basic " + authentication + "\r\n\r\n").encode())
                    reply = stream.recv(4096)
                    result[field] = b" 200 " not in reply.split(b"\r\n", 1)[0]
            except OSError:
                pass
    elif name == "remember-capability":
        token = os.environ.get("POCKYMOE_TOKEN", "")
        fd = os.open("/pockymoe/state/fixture-old-capability", os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as file:
            file.write(token)
        result["rememberedPrivately"] = bool(token)
    elif name == "stale-capability":
        token = Path("/pockymoe/state/fixture-old-capability").read_text()
        body = {"operation": "status", "threadId": os.environ.get("POCKYMOE_THREAD_ID")}
        result["staleCapabilityDenied"] = gateway(body, token=token) >= 400
        result["currentCapabilityWorks"] = gateway(body) == 200
    elif name == "wait":
        # A real native descendant lets cancellation tests inspect process-domain cleanup.
        child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(300)"], cwd=cwd,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        CHILDREN.append(child)
        (cwd / "cancel-native.pid").write_text(str(child.pid))
        report(session_id, {"case": "wait", "waiting": True, "nativeChildStarted": True})
        cancel.wait(300)
        if cancel.is_set():
            raise InterruptedError
    elif name == "once":
        path = cwd / "side-effect-count.txt"
        count = int(path.read_text()) if path.exists() else 0
        path.write_text(str(count + 1))
        result["effectCount"] = count + 1
    else:
        result["unsupportedCase"] = True
    return result


def prompt(message):
    params = message.get("params") or {}
    session_id = params.get("sessionId")
    cancel = threading.Event()
    ACTIVE[session_id] = cancel
    case = {}
    try:
        text = "".join(block.get("text", "") for block in params.get("prompt", []) if block.get("type") == "text")
        if MARKER in text:
            case, _ = json.JSONDecoder().raw_decode(text.rsplit(MARKER, 1)[1])
        else:
            case = {"case": "identity"}
        result = execute(case, session_id, cancel)
        report(session_id, result)
        response(message["id"], {"stopReason": "end_turn"})
    except InterruptedError:
        response(message["id"], {"stopReason": "cancelled"})
    except Exception as error:
        # Do not expose exception text: subprocess errors can carry env/argv.
        report(session_id, {"case": "fixture-error", "ok": False, "requestedCase": case.get("case"),
            "caseId": case.get("caseId"), "failureType": type(error).__name__})
        response(message["id"], {"stopReason": "end_turn"})
    finally:
        ACTIVE.pop(session_id, None)


def main():
    if "--version" in sys.argv:
        print("pocky-environment-acp 0.1.0")
        return
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if "method" not in message:
            with PENDING_LOCK:
                channel = PENDING.get(message.get("id"))
            if channel:
                channel.put_nowait(message)
            continue
        method = message["method"]
        params = message.get("params") or {}
        if method == "initialize":
            response(message["id"], {"protocolVersion": 1, "agentInfo": {"name": "pocky-environment-acp", "version": "0.1.0"},
                "agentCapabilities": {"loadSession": True, "promptCapabilities": {}}})
        elif method in {"session/new", "session/load"}:
            sid = params.get("sessionId") or "fixture-" + uuid.uuid4().hex
            SESSIONS[sid] = params.get("cwd") or os.getcwd()
            response(message["id"], {"sessionId": sid, "models": {"currentModelId": "fixture",
                "availableModels": [{"modelId": "fixture", "name": "Environment fixture"}]}})
        elif method == "session/prompt":
            text = "".join(block.get("text", "") for block in params.get("prompt", []) if block.get("type") == "text")
            try:
                case, _ = json.JSONDecoder().raw_decode(text.rsplit(MARKER, 1)[1])
            except (ValueError, IndexError):
                case = {}
            if case.get("case") == "blocked-stdin-writers" and os.environ.get("POCKYMOE_ENVIRONMENT_WORKER") == "1":
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                signal.signal(signal.SIGINT, signal.SIG_IGN)
                prompt(message)
            else:
                threading.Thread(target=prompt, args=(message,), daemon=True).start()
        elif method == "session/cancel":
            if params.get("sessionId") in ACTIVE:
                ACTIVE[params["sessionId"]].set()
            for child in CHILDREN:
                if child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=2)
            if "id" in message:
                response(message["id"], {})
        elif method in {"session/set_mode", "session/set_model", "session/set_config_option", "authenticate"}:
            response(message["id"], {})
        elif "id" in message:
            send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unsupported fixture method"}})


if __name__ == "__main__":
    try:
        main()
    finally:
        for child in CHILDREN:
            if child.poll() is None:
                child.terminate()
