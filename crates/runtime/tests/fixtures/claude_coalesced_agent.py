"""Model SDK autonomous coalescing and ACP's stranded queued prompt, with no tools/model calls."""
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid
from datetime import datetime, timezone

session = str(uuid.uuid4())
path = Path(os.environ["CLAUDE_CONFIG_DIR"]) / "projects" / "fixture" / (session + ".jsonl")
path.parent.mkdir(parents=True, exist_ok=True)
path.touch()
lock = threading.Lock()
pending = None
scenario = None


def send(value):
    with lock:
        print(json.dumps({"jsonrpc": "2.0", **value}), flush=True)


def native(value):
    with path.open("a") as stream:
        stream.write(json.dumps({"sessionId": session, "timestamp": datetime.now(timezone.utc).isoformat(), **value}) + "\n")


def raw(value):
    send({"method": "_claude/sdkMessage", "params": {"sessionId": session, "message": value}})


def reply(text):
    native({"type": "assistant", "message": {"stop_reason": "end_turn", "content": [{"type": "text", "text": text}]}})
    send({"method": "session/update", "params": {"sessionId": session, "update": {
        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})


def release_background():
    while not path.with_suffix(".release").exists():
        time.sleep(0.02)
    raw({"type": "system", "subtype": "background_tasks_changed", "tasks": []})


def finish_steering():
    global pending
    # Keep the old native completion/idle proof visible after steering is
    # accepted, reproducing the race before the SDK echoes the new input.
    # The hold must outlast the runtime's drain delay; tests that shorten that
    # delay shorten the hold to match.
    time.sleep(float(os.environ.get("POCKYMOE_COALESCED_STEER_HOLD_SECS", "5")))
    native({"type": "user", "uuid": str(uuid.uuid4()), "message": {"content": "change direction"}})
    reply("steering processed")
    if pending is not None:
        send({"id": pending, "result": {"stopReason": "end_turn"}})
        pending = None


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    request_id = request.get("id")
    if method == "session/cancel":
        path.with_suffix(".cancelled").touch()
        if pending is not None:
            if scenario == "cancel-error":
                send({"id": pending, "error": {"code": -32800, "message": "Request cancelled"}})
            else:
                send({"id": pending, "result": {"stopReason": "cancelled"}})
            pending = None
    elif method == "initialize":
        send({"id": request_id, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}, "_meta": {"steering": {"supported": True}}}})
    elif method in ("session/new", "session/load"):
        assert request["params"]["_meta"]["claudeCode"]["emitRawSDKMessages"]
        send({"id": request_id, "result": {"sessionId": session}})
    elif method == "session/prompt":
        prompt = request["params"]["prompt"][0]["text"]
        if prompt == "follow-up":
            reply("follow-up completed once")
            send({"id": request_id, "result": {"stopReason": "end_turn"}})
            continue
        scenario = prompt
        pending = request_id
        command = str(uuid.uuid4())
        raw({"type": "system", "subtype": "init"})
        if scenario == "background":
            # A previous-turn task must prevent cancellation even though the
            # current native turn has no Agent launches at all.
            raw({"type": "system", "subtype": "task_started", "task_id": "older-agent"})
            threading.Thread(target=release_background, daemon=True).start()
        native({"type": "attachment", "attachment": {"type": "queued_command", "source_uuid": command,
            "origin": {"kind": "human"}, "humanTurn": True,
            "prompt": [{"type": "text", "text": "unrelated" if scenario == "mismatch" else prompt}]}})
        raw({"type": "command_lifecycle", "command_uuid": command, "state": "started"})
        if scenario == "unfinished-tool":
            native({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_real", "name": "Bash"}]}})
        reply("done")
        if scenario != "missing-lifecycle":
            raw({"type": "command_lifecycle", "command_uuid": command, "state": "completed"})
        raw({"type": "result", "subtype": "error_during_execution" if scenario == "provider-error" else "success",
            "is_error": scenario == "provider-error", "num_turns": 1, "origin": {"kind": "task-notification"}})
        raw({"type": "system", "subtype": "session_state_changed", "state": "idle"})
        # Deliberately leave session/prompt unresolved until an explicit cancel.
    elif method == "_session/steering":
        send({"id": request_id, "result": {"outcome": "injected"}})
        threading.Thread(target=finish_steering, daemon=True).start()
    elif request_id is not None:
        send({"id": request_id, "result": {}})
