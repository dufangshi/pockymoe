#!/usr/bin/env python3
"""Check the ACP test agent itself; this is not product integration evidence."""
import argparse
import json
import queue
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path
sys.dont_write_bytecode = True

from verify import Evidence, HERE, basic_env, find_results, git, load_state, unused_port, write_json
from wire import WebSocket


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()
    state, root = load_state(args.manifest)
    directory = root / "fixture-selfcheck"
    directory.mkdir(exist_ok=True)
    git(directory, "init", "-q", "-b", "main")
    git(directory, "rm", "-f", "--ignore-unmatch", "agent-change.txt")
    (directory / "tracked.txt").write_text("baseline\n")
    git(directory, "add", "tracked.txt")
    git(directory, "commit", "--allow-empty", "-qm", "Self-check baseline")
    evidence = Evidence(directory)
    agent = subprocess.Popen([sys.executable, str(HERE / "acp_agent.py")], cwd=directory, env=basic_env(root / "home"),
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
    incoming = queue.Queue()

    def read():
        for line in agent.stdout:
            incoming.put(json.loads(line))

    threading.Thread(target=read, daemon=True).start()
    next_id = 0
    terminals = {}

    def send(value):
        agent.stdin.write(json.dumps(value) + "\n")
        agent.stdin.flush()

    def handle_callback(message):
        params = message["params"]
        method = message["method"]
        value = {}
        if method == "fs/write_text_file":
            Path(params["path"]).write_text(params["content"])
        elif method == "fs/read_text_file":
            value = {"content": Path(params["path"]).read_text()}
        elif method == "terminal/create":
            child = subprocess.Popen([params["command"], *params["args"]], cwd=params["cwd"],
                env=basic_env(root / "home"), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            terminals["terminal"] = child
            value = {"terminalId": "terminal"}
        elif method == "terminal/wait_for_exit":
            terminals["terminal"].wait(timeout=10)
            value = {"exitCode": 0}
        elif method == "terminal/output":
            value = {"output": terminals["terminal"].stdout.read().decode(), "exitStatus": {"exitCode": 0}}
        elif method == "terminal/release":
            pass
        else:
            raise AssertionError("Unexpected fixture callback")
        send({"jsonrpc": "2.0", "id": message["id"], "result": value})

    def request(method, params):
        nonlocal next_id
        next_id += 1
        request_id = next_id
        send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        results = []
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            message = incoming.get(timeout=max(0.01, deadline - time.monotonic()))
            if "method" in message and "id" in message:
                handle_callback(message)
            elif message.get("id") == request_id:
                return message["result"], results
            else:
                results.extend(find_results(message))
        raise AssertionError("Fixture response timed out")

    try:
        initialized, _ = request("initialize", {"protocolVersion": 1})
        evidence.check("fixture-initialize", initialized["protocolVersion"] == 1)
        session, _ = request("session/new", {"cwd": str(directory), "mcpServers": []})
        sid = session["sessionId"]
        for case, fields in [("fs", ["writeOk", "readMatches"]), ("native", ["nativeSubprocessOk"]),
                ("terminal", ["created", "completed"]), ("git", ["added", "committed"])]:
            _, results = request("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": "POCKY_ENV_CASE " + json.dumps({"case": case})}]})
            result = results[-1]
            evidence.check("fixture-" + case, all(result.get(field) is True for field in fields))
        port = unused_port()
        _, results = request("session/prompt", {"sessionId": sid, "prompt": [{"type": "text", "text": "POCKY_ENV_CASE " + json.dumps({"case": "service", "port": port, "label": "selfcheck"})}]})
        evidence.check("fixture-service-start", results[-1].get("serviceReady") is True)
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=5) as response:
            evidence.check("fixture-origin-http", json.load(response)["label"] == "selfcheck")
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/sse", timeout=5) as response:
            evidence.check("fixture-origin-sse", response.read().count(b"event: fixture") == 3)
        with WebSocket(f"ws://127.0.0.1:{port}/ws") as websocket:
            websocket.send("ping")
            evidence.check("fixture-origin-websocket", websocket.receive() == "selfcheck:ping")
        next_id += 1
        waiting_id = next_id
        send({"jsonrpc": "2.0", "id": waiting_id, "method": "session/prompt", "params": {"sessionId": sid,
            "prompt": [{"type": "text", "text": 'POCKY_ENV_CASE {"case":"wait"}'}]}})
        while True:
            message = incoming.get(timeout=8)
            if any(result.get("waiting") for result in find_results(message)):
                break
        child_pid = (directory / "cancel-native.pid").read_text().strip()
        send({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}})
        while True:
            message = incoming.get(timeout=8)
            if message.get("id") == waiting_id:
                evidence.check("fixture-cancel", message["result"]["stopReason"] == "cancelled")
                break
        deadline = time.monotonic() + 4
        while Path("/proc", child_pid).exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        evidence.check("fixture-cancel-reaps-native-descendant", not Path("/proc", child_pid).exists())
    finally:
        agent.stdin.close()
        try:
            agent.wait(timeout=5)
        except subprocess.TimeoutExpired:
            agent.kill()
            agent.wait(timeout=5)
    passed = all(item["passed"] for item in evidence.checks)
    write_json(directory / "summary.json", {"passed": passed, "checks": len(evidence.checks), "boundary": "ACP fixture self-check only; not a Rust/provider result"})
    print(json.dumps({"fixtureSelfCheckPassed": passed, "evidence": str(directory / "summary.json")}))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
