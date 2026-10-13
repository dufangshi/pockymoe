#!/usr/bin/env python3
"""ACP agent for harness release tests (Unix).

Advertises session/close unless started with --no-close (--hang-close never
answers it), logs every close to closed.jsonl, and starts tool processes that
ignore SIGTERM: one in the agent's process group, one in its own session as
Claude's Bash commands run, one in its own process group as
codex-code-mode-host does.
"""
import json
import os
import subprocess
import sys
import uuid


def respond(request_id, result=None, error=None):
    message = {"jsonrpc": "2.0", "id": request_id}
    if error is None:
        message["result"] = result
    else:
        message["error"] = {"code": -32601, "message": error}
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params") or {}
    if request_id is None:
        continue
    if method == "initialize":
        session = {} if "--no-close" in sys.argv else {"close": {}}
        respond(request_id, {
            "protocolVersion": 1,
            "agentCapabilities": {"loadSession": True, "sessionCapabilities": session},
        })
    elif method in ("session/new", "session/load"):
        if not os.path.exists("tool.pid"):
            stubborn = [
                sys.executable, "-c",
                "import signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); time.sleep(300)",
            ]
            for name, detach in (("tool", None), ("session", os.setsid), ("group", os.setpgrp)):
                tool = subprocess.Popen(stubborn, preexec_fn=detach)
                with open(name + ".pid", "w") as output:
                    output.write(str(tool.pid))
        respond(request_id, {"sessionId": params.get("sessionId") or str(uuid.uuid4())})
    elif method == "session/close":
        with open("closed.jsonl", "a") as output:
            output.write(json.dumps(params) + "\n")
        if "--hang-close" not in sys.argv:
            respond(request_id, {})
    elif method == "session/prompt":
        respond(request_id, {"stopReason": "end_turn"})
    else:
        respond(request_id, error="unsupported method: " + str(method))
