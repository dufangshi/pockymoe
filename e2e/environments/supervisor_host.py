#!/usr/bin/env python3
"""Stable disposable lab host for real Supervisor SIGKILL/restart tests.

Unlike killing container PID 1 (which destroys its entire PID namespace), this
keeps a service-host process alive while the real Supervisor crashes. No product
recovery hooks or credentials are added. Only public PID/restart metadata lives
in the supplied task-private file.
"""
import argparse, json, signal, subprocess, time
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("--binary", required=True)
p.add_argument("--metadata", type=Path, required=True)
a = p.parse_args()
stop = False


def terminate(*_):
    global stop
    stop = True


signal.signal(signal.SIGTERM, terminate)
signal.signal(signal.SIGINT, terminate)
count = 0
while not stop:
    child = subprocess.Popen([a.binary, "supervisor"])
    a.metadata.write_text(
        json.dumps({"supervisorPid": child.pid, "restartCount": count}) + "\n"
    )
    while child.poll() is None and not stop:
        time.sleep(0.03)
    if stop:
        child.terminate()
        try:
            child.wait(timeout=15)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()
    else:
        count += 1
        time.sleep(0.1)
