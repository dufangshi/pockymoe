"""Replays a recorded Claude Code stream-json session (real CLI frames, sanitized).

Frames marked `"_dir": "in"` are what the recorder sent; every other frame is
what Claude Code wrote. The replay waits for the runtime under test to send
the same kind of input (a prompt, a control request of the same subtype, or
the reply to a recorded callback) before it writes the frames that followed
it. Recorded prompt UUIDs are rewritten to the runtime's own, so results name
the runtime's prompts as the real CLI would. Usage: replay.py FIXTURE
"""
import json, os, sys

frames = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
log = os.environ.get("REPLAY_LOG")
renames = {}
pending = []  # inputs read early (a steer before its recorded point)


def emit(frame):
    text = json.dumps({k: v for k, v in frame.items() if k != "_dir"})
    for old, new in renames.items():
        text = text.replace(old, new)
    sys.stdout.write(text + "\n")
    sys.stdout.flush()


def read():
    if pending:
        return pending.pop(0)
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    value = json.loads(line)
    if log:
        with open(log, "a") as f:
            f.write(line)
    return value


def matches(expected, got):
    if expected["type"] != got["type"]:
        return False
    if expected["type"] == "control_request":
        return expected["request"]["subtype"] == got["request"]["subtype"]
    if expected["type"] == "control_response":
        return expected["response"]["request_id"] == got["response"]["request_id"]
    return True


def responses(request_id):
    return [
        f for f in frames
        if f.get("_dir") != "in" and f["type"] == "control_response"
        and f["response"].get("request_id") == request_id
    ]


for frame in frames:
    if frame.get("_dir") == "in":
        skipped = []
        while True:
            got = read()
            if matches(frame, got):
                break
            if got["type"] == "control_request":
                # An unrecorded control request: answer it without data.
                sys.stdout.write(json.dumps({"type": "control_response", "response": {
                    "subtype": "success", "request_id": got["request_id"], "response": {}}}) + "\n")
                sys.stdout.flush()
            else:
                skipped.append(got)
        pending[:0] = skipped
        if frame["type"] == "user" and frame.get("uuid") and got.get("uuid"):
            renames[frame["uuid"]] = got["uuid"]
        if frame["type"] == "control_request":
            for response in responses(frame["request_id"]):
                response = json.loads(json.dumps(response))
                response["response"]["request_id"] = got["request_id"]
                emit(response)
        continue
    if frame["type"] == "control_response":
        continue  # emitted when its recorded request arrives
    emit(frame)

# Keep the process alive like the real CLI until stdin closes.
while sys.stdin.readline():
    pass
