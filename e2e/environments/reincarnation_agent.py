#!/usr/bin/env python3
"""ACP protocol fixture for an owner-approved same-thread migration. No LLM."""
import itertools, json, os, queue, subprocess, sys, threading, uuid, urllib.request, urllib.error
from pathlib import Path

LOCK = threading.Lock()
PENDING = {}
SESSIONS = {}
IDS = itertools.count(700000)
ACTIVE = {}


def options():
    return [
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": "fixture",
            "options": [{"value": "fixture", "name": "Reincarnation fixture"}],
        },
        {
            "id": "reasoning_effort",
            "name": "Reasoning effort",
            "category": "thought_level",
            "type": "select",
            "currentValue": "max",
            "options": [{"value": "max", "name": "Max"}],
        },
    ]


def send(v):
    with LOCK:
        print(json.dumps(v, separators=(",", ":")), flush=True)


def reply(i, v):
    send({"jsonrpc": "2.0", "id": i, "result": v})


def message(s, v):
    send(
        {
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": s,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {
                        "type": "text",
                        "text": "POCKY_REINCARNATION_RESULT "
                        + json.dumps(v, separators=(",", ":")),
                    },
                },
            },
        }
    )


def cli(cwd, *args):
    binary = (
        "/opt/pockymoe/bin/pockymoe"
        if os.environ.get("POCKYMOE_ENVIRONMENT_WORKER")
        else "pockymoe"
    )
    r = subprocess.run([binary, *args], cwd=cwd, capture_output=True, timeout=30)
    if r.returncode:
        raise RuntimeError("Fixture CLI failed; output withheld")
    return json.loads(r.stdout)


def callback(method, params):
    i = next(IDS)
    q = queue.Queue()
    PENDING[i] = q
    send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})
    try:
        return q.get(timeout=20)
    finally:
        PENDING.pop(i, None)


def forbidden(path, body):
    try:
        req = urllib.request.Request(
            os.environ["POCKYMOE_URL"] + path,
            data=json.dumps(body).encode(),
            headers={
                "Content-Type": "application/json",
                "Authorization": "Bearer " + os.environ["POCKYMOE_TOKEN"],
            },
        )
        r = urllib.request.urlopen(req, timeout=8)
        r.read()
        return False
    except urllib.error.HTTPError as e:
        e.read()
        return e.code in [401, 403, 404]


def turn(i, p):
    sid = p["sessionId"]
    cwd = Path(SESSIONS[sid])
    cancel = threading.Event()
    ACTIVE[sid] = cancel
    try:
        text = "\n".join(
            v.get("text", "") for v in p.get("prompt", []) if v.get("type") == "text"
        )
        isolated = bool(os.environ.get("POCKYMOE_ENVIRONMENT_WORKER"))
        if not isolated and "fixture: propose" in text:
            proof = "import sys,os,json,pathlib;pathlib.Path('controlled-host-proof.json').write_text(json.dumps({'stdinEof':sys.stdin.read()=='','ambientRemoved':'HOST_GUARDIAN_UNRELATED' not in os.environ,'identityRemoved':'POCKYMOE_TOKEN' not in os.environ}))"
            cli(
                cwd,
                "command",
                "run",
                "--request-id",
                "host-compatibility-proof",
                "--",
                "python3",
                "-c",
                proof,
            )
            receipt = cli(
                cwd,
                "environment",
                "request",
                "--spec",
                str(cwd / ".fixture/spec.json"),
                "--request-id",
                "owner-migration-fixture",
            )
            (cwd / "proposal-receipt.json").write_text(json.dumps(receipt))
            duplicate = cli(
                cwd,
                "environment",
                "request",
                "--spec",
                str(cwd / ".fixture/spec.json"),
                "--request-id",
                "owner-migration-fixture",
            )
            discovery = cli(cwd, "environment", "connections")
            shown = cli(cwd, "environment", "show", receipt["id"])
            listed = cli(cwd, "environment", "requests")
            denied = {
                "agentOwnerHttpDenied": forbidden(
                    "/api/environment-requests/" + receipt["id"] + "/approve",
                    {"digest": receipt["digest"]},
                ),
                "forgedThreadDenied": forbidden(
                    "/api/cli",
                    {
                        "operation": "environmentRequestShow",
                        "requestId": receipt["id"],
                        "fromThreadId": str(uuid.uuid4()),
                    },
                ),
                "requestIdempotent": duplicate["id"] == receipt["id"]
                and duplicate["digest"] == receipt["digest"],
                "cliDiscoveryShowList": bool(discovery.get("connections"))
                and shown["id"] == receipt["id"]
                and any(r["id"] == receipt["id"] for r in listed),
            }
            (cwd / "agent-request-security.json").write_text(json.dumps(denied))
            finite = callback(
                "terminal/create",
                {
                    "sessionId": sid,
                    "command": "python3",
                    "args": [
                        "-c",
                        "import sys,pathlib;pathlib.Path('callback-stdin-eof.txt').write_text(str(sys.stdin.read()==''))",
                    ],
                    "cwd": str(cwd),
                    "env": [],
                },
            )
            if "result" in finite:
                callback(
                    "terminal/wait_for_exit",
                    {"sessionId": sid, "terminalId": finite["result"]["terminalId"]},
                )
            # This writer changes session/process group; the guardian must still own it.
            subprocess.Popen(
                [
                    "python3",
                    "-c",
                    "import os,time,pathlib;os.setsid();p=pathlib.Path('host-writer-count');n=0\nwhile True:\n n+=1;p.write_text(str(n));time.sleep(.05)",
                ],
                cwd=cwd,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            terminal = callback(
                "terminal/create",
                {
                    "sessionId": sid,
                    "command": "python3",
                    "args": [
                        "-c",
                        "import time,pathlib\np=pathlib.Path('callback-writer-count');n=0\nwhile True:\n n+=1;p.write_text(str(n));time.sleep(.05)",
                    ],
                    "cwd": str(cwd),
                    "env": [],
                },
            )
            message(
                sid,
                {
                    "requestSubmitted": True,
                    "pendingWithoutBlockingCli": receipt.get("state") == "pending",
                    "threadId": receipt.get("threadId"),
                    "callbackStarted": "result" in terminal,
                },
            )
            while not (cwd / "allow-host-finish").exists() and not cancel.wait(0.03):
                if (cwd / "cancel-request").exists() and not (
                    cwd / "cancel-receipt.json"
                ).exists():
                    (cwd / "cancel-receipt.json").write_text(
                        json.dumps(cli(cwd, "environment", "cancel", receipt["id"]))
                    )
            message(sid, {"oldTurnFinished": not cancel.is_set()})
        elif isolated:
            identity = cli(cwd, "thread", "self")
            expected = os.environ.get("POCKYMOE_THREAD_ID")
            response = callback(
                "fs/write_text_file",
                {
                    "sessionId": sid,
                    "path": str(cwd / "isolated-callback.txt"),
                    "content": "original storage modified through isolated ACP callback\n",
                },
            )
            worktree = cli(
                cwd,
                "worktree",
                "create",
                "--name",
                "reincarnation-child",
                "--request-id",
                "reincarnation-child",
            )
            counts = cwd / "continuation-count"
            count = int(counts.read_text()) + 1 if counts.exists() else 1
            counts.write_text(str(count))
            (cwd / "isolated-native.txt").write_text(
                "original storage modified by isolated native tool\n"
            )
            git = subprocess.run(
                [
                    "git",
                    "-c",
                    "user.name=Reincarnation Fixture",
                    "-c",
                    "user.email=fixture@test.invalid",
                    "add",
                    "isolated-callback.txt",
                    "isolated-native.txt",
                ],
                cwd=cwd,
                capture_output=True,
            )
            commit = subprocess.run(
                [
                    "git",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "user.name=Reincarnation Fixture",
                    "-c",
                    "user.email=fixture@test.invalid",
                    "commit",
                    "-m",
                    "isolated same-thread fixture",
                ],
                cwd=cwd,
                capture_output=True,
            )
            result = {
                "isolated": True,
                "sameThread": identity.get("threadId") == expected,
                "threadId": identity.get("threadId"),
                "summaryPreservedOwnerConstraint": "never modify preserved.txt" in text,
                "handoffUntrusted": "Agent handoff (untrusted context" in text,
                "callbackWrite": "result" in response,
                "managedWorktree": bool(worktree.get("workingCopy")),
                "gitCommit": git.returncode == 0 and commit.returncode == 0,
                "continuationCount": count,
                "nativeSessionId": sid,
            }
            result["gatewayCannotApprove"] = forbidden(
                "/api/environment-requests/invalid/approve", {"digest": "f" * 64}
            )
            (cwd / "continuation-evidence.json").write_text(json.dumps(result))
            message(sid, result)
            if (cwd / ".fixture/native_validation.py").exists():
                probe = subprocess.run(
                    ["python3", str(cwd / ".fixture/native_validation.py")],
                    cwd=cwd,
                    capture_output=True,
                    timeout=170,
                )
                (cwd / "native-probe-exit.json").write_text(
                    json.dumps({"exitSuccess": probe.returncode == 0})
                )
        else:
            message(sid, {"hostReady": True})
        reply(i, {"stopReason": "cancelled" if cancel.is_set() else "end_turn"})
    except Exception as e:
        message(sid, {"fixtureFailed": True, "category": type(e).__name__})
        reply(i, {"stopReason": "end_turn"})
    finally:
        ACTIVE.pop(sid, None)


def main():
    for line in sys.stdin:
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if "method" not in r:
            if r.get("id") in PENDING:
                PENDING[r["id"]].put(r)
            continue
        method = r["method"]
        p = r.get("params") or {}
        i = r.get("id")
        if method == "initialize":
            reply(
                i,
                {
                    "protocolVersion": 1,
                    "agentInfo": {
                        "name": "pockymoe-reincarnation-fixture",
                        "version": "1.0.0",
                    },
                    "agentCapabilities": {"loadSession": False},
                    "authMethods": [],
                },
            )
        elif method == "session/new":
            sid = str(uuid.uuid4())
            SESSIONS[sid] = p["cwd"]
            reply(
                i,
                {
                    "sessionId": sid,
                    "models": {
                        "currentModelId": "fixture",
                        "availableModels": [
                            {"modelId": "fixture", "name": "Reincarnation fixture"}
                        ],
                    },
                    "configOptions": options(),
                },
            )
        elif method == "session/prompt":
            threading.Thread(target=turn, args=(i, p), daemon=True).start()
        elif method == "session/cancel":
            if p.get("sessionId") in ACTIVE:
                ACTIVE[p["sessionId"]].set()
        elif method == "session/set_config_option":
            reply(i, {"configOptions": options()})
        elif method in ["session/set_mode", "session/set_model", "authenticate"]:
            reply(i, {})
        elif i is not None:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": i,
                    "error": {"code": -32601, "message": "Unsupported fixture method"},
                }
            )


if __name__ == "__main__":
    main()
