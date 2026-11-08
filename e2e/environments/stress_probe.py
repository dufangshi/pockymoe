#!/usr/bin/env python3
"""Bounded cgroup probes, run only inside disposable test containers."""
import argparse
import errno
import json
import os
import signal
import time
from pathlib import Path


def counters(name):
    return dict((key, int(value)) for key, value in (line.split() for line in Path("/sys/fs/cgroup/" + name).read_text().splitlines()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["cpu", "pids", "oom"])
    args = parser.parse_args()
    result = {"mode": args.mode, "memoryMax": Path("/sys/fs/cgroup/memory.max").read_text().strip(),
        "pidsMax": Path("/sys/fs/cgroup/pids.max").read_text().strip(), "cpuMax": Path("/sys/fs/cgroup/cpu.max").read_text().strip()}
    if args.mode == "cpu":
        before = counters("cpu.stat")
        deadline = time.monotonic() + 2
        value = 1
        while time.monotonic() < deadline:
            value = (value * 1664525 + 1013904223) & 0xFFFFFFFF
        after = counters("cpu.stat")
        result.update({"nrThrottledDelta": after.get("nr_throttled", 0) - before.get("nr_throttled", 0),
            "throttledUsecDelta": after.get("throttled_usec", 0) - before.get("throttled_usec", 0)})
    elif args.mode == "pids":
        children = []
        limited = False
        try:
            for _ in range(96):
                try:
                    pid = os.fork()
                except OSError as error:
                    limited = error.errno == errno.EAGAIN
                    break
                if pid == 0:
                    time.sleep(8)
                    os._exit(0)
                children.append(pid)
            result.update({"pidsLimited": limited, "childrenBeforeLimit": len(children)})
        finally:
            for pid in children:
                try:
                    os.kill(pid, signal.SIGTERM)
                    os.waitpid(pid, 0)
                except (OSError, ChildProcessError):
                    pass
    elif args.mode == "oom":
        # Never run this on the host: explicit 64 MiB cgroup limit is mandatory.
        if result["memoryMax"] != "67108864":
            raise SystemExit("OOM probe requires the exact disposable cgroup budget")
        blocks = []
        for _ in range(32):
            blocks.append(bytearray(8 * 1024 * 1024))
            time.sleep(0.01)
        raise SystemExit("Expected cgroup OOM did not occur")
    Path("/probe/result.json").write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
