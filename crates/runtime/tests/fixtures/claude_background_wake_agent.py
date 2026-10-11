"""An ACP foreground reply settles before a native Bash task wakes Claude."""
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid
from datetime import datetime, timezone

session = str(uuid.uuid4())
root = Path.cwd()
lock = threading.Lock()
cancelled = threading.Event()
foreground_done = False
correction_request = None
rollout = Path(os.environ["CLAUDE_CONFIG_DIR"]) / "projects" / "wake-fixture" / (session + ".jsonl")
rollout.parent.mkdir(parents=True, exist_ok=True)
rollout.touch()


def native(value):
    with rollout.open("a") as stream:
        stream.write(json.dumps({"sessionId": session, "timestamp": datetime.now(timezone.utc).isoformat(), **value}) + "\n")


def usage(message_id, output):
    with rollout.open("a") as stream:
        stream.write(json.dumps({"type": "assistant", "sessionId": session,
            "timestamp": datetime.now(timezone.utc).isoformat(), "message": {
                "id": message_id, "model": "claude-opus-4-6",
                "usage": {"input_tokens": 10, "output_tokens": output}}}) + "\n")


def send(value):
    with lock:
        print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def raw(message, sid=None):
    send({"method": "_claude/sdkMessage", "params": {"sessionId": sid or session, "message": message}})


def update(value):
    send({"method": "session/update", "params": {"sessionId": session, "update": value}})


def wait_for(name):
    while not (root / name).exists():
        if cancelled.wait(0.01):
            return False
    return True


def background(scenario):
    if not wait_for("release"):
        return
    notification = {"type": "system", "subtype": "task_notification", "task_id": "release-watch",
                    "tool_use_id": "watch-tool", "status": "completed", "summary": "GitHub release finished"}
    raw(notification, "other-session")
    if scenario in ("native", "monitor", "idle-native"):
        tag = "<summary>Monitor event: GitHub progress</summary><event>Linux completed</event>" if scenario == "monitor" else "<status>completed</status><summary>GitHub release finished</summary>"
        notice = {"type": "user", "uuid": str(uuid.uuid4()), "origin": {"kind": "task-notification"}, "message": {"content":
            "<task-notification><task-id>release-watch</task-id>" + tag + "</task-notification>"}}
        native(notice)
        native(notice)
    else:
        raw(notification)
        raw(notification)  # SDK retries must not duplicate the wake marker.
    # Real SDK repeats init at the beginning of autonomous cycles.
    raw({"type": "system", "subtype": "session_state_changed", "state": "running"})
    raw({"type": "system", "subtype": "init"})
    if scenario not in ("native", "monitor", "idle-native"):
        raw({"type": "system", "subtype": "background_tasks_changed", "tasks": []})
    usage("followup", 1)
    usage("followup", 5)  # Snapshots of one model request count only once.
    update({"sessionUpdate": "agent_message_chunk", "messageId": "followup", "content": {
        "type": "text", "text": "Checking the completed release."}})
    update({"sessionUpdate": "tool_call", "toolCallId": "verify-release", "kind": "execute",
            "title": "Verify release", "status": "pending", "rawInput": {"command": "verify-release"}})
    (root / "followup-started").touch()
    if not wait_for("finish"):
        return
    if scenario == "disconnect":
        os._exit(0)
    update({"sessionUpdate": "tool_call_update", "toolCallId": "verify-release",
            "status": "completed", "rawOutput": "Verified without repeating the release"})
    if scenario == "monitor":
        native({"type":"user","origin":{"kind":"task-notification"},"message":{"content":"<task-notification><task-id>release-watch</task-id><status>completed</status><summary>Monitor finished</summary></task-notification>"}})
    if scenario != "silent":
        usage("final", 3)
        native({"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Release verified."}]}})
        update({"sessionUpdate": "agent_message_chunk", "messageId": "final", "content": {
            "type": "text", "text": "Release verified."}})
    if scenario in ("native", "monitor", "idle-native"):
        return
    raw({"type": "result", "subtype": "success", "is_error": False, "num_turns": 1,
         "origin": {"kind": "task-notification"}})
    raw({"type": "system", "subtype": "session_state_changed", "state": "idle"})
    raw(notification)  # A late duplicate cannot reopen an already-idle task.


for line in sys.stdin:
    request = json.loads(line)
    method, ident = request.get("method"), request.get("id")
    if method == "initialize":
        send({"id": ident, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True},
                                      "_meta": {"steering": {"supported": True}}}})
    elif method in ("session/new", "session/load"):
        send({"id": ident, "result": {"sessionId": session}})
    elif method == "session/prompt":
        if foreground_done:
            correction_request = ident
            text = request["params"]["prompt"][0]["text"]
            native({"type":"user","message":{"content":text}})
            raw({"type":"system","subtype":"session_state_changed","state":"running"})
            update({"sessionUpdate":"agent_message_chunk","messageId":"idle-steering","content":{"type":"text","text":"Waiting correction processed."}})
            (root / "idle-steering-started").touch()
            def finish_correction(request_id):
                if not wait_for("finish-correction"):
                    return
                raw({"type":"system","subtype":"session_state_changed","state":"idle"})
                send({"id":request_id,"result":{"stopReason":"end_turn"}})
            threading.Thread(target=finish_correction, args=(ident,), daemon=True).start()
            continue
        scenario = request["params"]["prompt"][0]["text"]
        raw({"type": "system", "subtype": "init"})
        if scenario in ("native", "monitor", "idle-native"):
            name = "Monitor" if scenario == "monitor" else "Bash"
            receipt = {"taskId":"release-watch"} if name == "Monitor" else {"backgroundTaskId":"release-watch"}
            native({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"id":"launch"}]}})
            native({"type":"user","toolUseResult":receipt,"message":{"content":[{"type":"tool_result","tool_use_id":"launch"}]}})
            native({"type":"assistant","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"Waiting for GitHub."}]}})
        else:
            raw({"type": "system", "subtype": "task_started", "task_id": "release-watch"})
        usage("initial", 2)
        update({"sessionUpdate": "agent_message_chunk", "messageId": "initial", "content": {
            "type": "text", "text": "Waiting for GitHub."}})
        raw({"type": "system", "subtype": "session_state_changed", "state": "idle"})
        send({"id": ident, "result": {"stopReason": "end_turn"}})
        foreground_done = True
        (root / "foreground-returned").touch()
        threading.Thread(target=background, args=(scenario,), daemon=True).start()
    elif method == "_session/steering":
        if request["params"]["prompt"][0]["text"] == "reject-waiting-steer":
            send({"id":ident,"error":{"code":-32602,"message":"invalid steering input"}})
            continue
        if not (root / "followup-started").exists():
            send({"id":ident,"result":{"outcome":"promptRequired","reason":"noRunningTurn"}})
            continue
        update({"sessionUpdate": "agent_message_chunk", "messageId": "steering", "content": {
            "type": "text", "text": "User correction processed."}})
        send({"id": ident, "result": {"outcome": "injected"}})
    elif method == "session/cancel":
        cancelled.set()
        (root / "cancelled").touch()
        if correction_request is not None:
            send({"id":correction_request,"result":{"stopReason":"cancelled"}})
    elif ident is not None:
        send({"id": ident, "result": {}})
