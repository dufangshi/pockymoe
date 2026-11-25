#!/usr/bin/env python3
"""Model Codex's writer locks and fork responses for ACP lifecycle tests (Unix)."""
import fcntl
import json
import os
import socket
import sys
import threading
import uuid

writers = {}
mutex = threading.Lock()


def acquire(sid):
    if sid in writers:
        return
    handle = open(sid + ".lock", "w")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        raise RuntimeError(f"thread {sid} already has an active writer")
    writers[sid] = handle


def dispatch(message):
    method = message.get("method")
    params = message.get("params", {})
    if method == "initialize":
        return {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}
    if method == "thread/turns/list":
        return {"data": [{"id": "turn-2"}, {"id": "turn-1"}], "nextCursor": None}
    if method == "thread/fork":
        sid = str(uuid.uuid4())
        acquire(sid)
        with open(sid + ".json", "w") as output:
            json.dump(params, output)
        turns = []
        if not params.get("excludeTurns"):
            # Model inherited history larger than the ACP transport limit.
            reply = {"type": "agentMessage", "text": "x" * (5 * 1024 * 1024)}
            turns.append({"items": [reply]})
        return {"thread": {"id": sid, "turns": turns}}
    if method == "thread/settings/update":
        return {}
    if method in ("session/new", "session/load"):
        sid = params.get("sessionId", str(uuid.uuid4()))
        acquire(sid)
        return {"sessionId": sid}
    if method == "session/prompt":
        sid = params["sessionId"]
        if sid not in writers:
            raise RuntimeError("session is not loaded")
        return {"stopReason": "end_turn"}
    raise RuntimeError("unsupported method: " + str(method))


def serve(reader, writer):
    for line in reader:
        message = json.loads(line)
        if "id" not in message:
            continue
        with mutex:
            with open("requests.jsonl", "a") as output:
                output.write(json.dumps({"pid": os.getpid(), **message}) + "\n")
            try:
                response = {"id": message["id"], "result": dispatch(message)}
            except RuntimeError as error:
                response = {"id": message["id"], "error": {"code": -32603, "message": str(error)}}
            writer.write(json.dumps(response) + "\n")
            writer.flush()


def bridge():
    config = json.loads(os.environ["POCKYMOE_APP_SERVER_BRIDGE"])
    host, port = config["address"].rsplit(":", 1)
    connection = socket.create_connection((host, int(port)))
    stream = connection.makefile("rw")
    stream.write(config["token"] + "\n")
    stream.flush()
    serve(stream, stream)


if "app-server" not in sys.argv:
    threading.Thread(target=bridge, daemon=True).start()
serve(sys.stdin, sys.stdout)
