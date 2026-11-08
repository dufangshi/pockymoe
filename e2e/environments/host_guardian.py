#!/usr/bin/env python3
"""Actual binary handshake/stdio/process-domain regression, no harness/network.

The parent is a registration coordinator fixture using the same on-disk receipt
and stdin handshake, not a mock guardian. Supervisor crash/restart is tested
separately by the real Dokploy acceptance driver.
"""
import argparse, json, subprocess, time, uuid
from pathlib import Path


def start_tick(pid):
    return Path("/proc", str(pid), "stat").read_text().rsplit(") ", 1)[1].split()[19]


def wait(fn, seconds=6):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if fn():
            return
        time.sleep(0.015)
    raise AssertionError("Guardian acceptance condition timed out")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary", type=Path, required=True)
    p.add_argument("--evidence", type=Path, required=True)
    a = p.parse_args()
    a.evidence = a.evidence.resolve()
    a.evidence.mkdir(parents=True, mode=0o700)
    e = []

    def check(name, value):
        assert value, "Guardian regression failed: " + name
        e.append({"name": name, "passed": True})
        (a.evidence / "evidence.json").write_text(json.dumps(e, indent=2) + "\n")
        print("PASS", name, flush=True)

    def launch(registered, argv, closed=False):
        ident = str(uuid.uuid4())
        path = a.evidence / (ident + ".json")
        receipt = {
            "threadId": "guardian-fixture",
            "id": ident,
            "pid": None,
            "start": None,
            "stopped": False,
        }
        path.write_text(json.dumps(receipt))
        path.chmod(0o600)
        env = {
            "PATH": "/usr/bin:/bin",
            "HOME": str(a.evidence),
            "POCKYMOE_HOST_PROCESS_RECEIPT": str(path),
            "HOST_GUARDIAN_SENTINEL": "public-inherited-setting",
        }
        c = subprocess.Popen(
            [str(a.binary.resolve()), "environment-worker", "host-run", ident],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            env=env,
        )
        if registered:
            receipt.update(pid=c.pid, start=start_tick(c.pid))
            path.write_text(json.dumps(receipt))
        c.stdin.write(
            (
                json.dumps(
                    {
                        "id": ident,
                        "argv": argv,
                        "cwd": str(a.evidence.resolve()),
                        "env": [],
                        "stdinClosed": closed,
                    }
                )
                + "\n"
            ).encode()
        )
        c.stdin.flush()
        return c, path

    def stopped(path):
        return json.loads(path.read_text())["stopped"]

    marker = a.evidence / "unregistered-must-not-run"
    c, path = launch(False, ["/bin/sh", "-c", "touch " + str(marker.resolve())])
    c.stdin.close()
    c.wait(timeout=5)
    check(
        "unregistered-receipt-never-executes", not marker.exists() and not stopped(path)
    )
    c, path = launch(
        True,
        [
            "/usr/bin/python3",
            "-c",
            "import os,sys;print('eof='+str(sys.stdin.read()==''));print('env='+str(os.getenv('HOST_GUARDIAN_SENTINEL')=='public-inherited-setting'));sys.exit(7)",
        ],
        True,
    )
    c.wait(timeout=5)
    data = c.stdout.read()
    check("closed-stdin-separate-from-live-transport", b"eof=True" in data)
    check("host-inherited-environment-preserved", b"env=True" in data)
    check("finite-exit-status-and-cleanup-receipt", c.returncode == 7 and stopped(path))
    c.stdin.close()
    started = a.evidence / "domain-started"
    progress = a.evidence / "domain-progress"
    code = "import os,time,pathlib;os.setsid();pathlib.Path('domain-started').touch();p=pathlib.Path('domain-progress');n=0\nwhile True:\n n+=1;p.write_text(str(n));time.sleep(.02)"
    parent = (
        "import subprocess,time;subprocess.Popen(['/usr/bin/python3','-c',"
        + repr(code)
        + "]);time.sleep(60)"
    )
    c, path = launch(True, ["/usr/bin/python3", "-c", parent])
    wait(started.exists)
    c.stdin.close()
    c.wait(timeout=6)
    before = progress.read_text()
    time.sleep(0.08)
    check(
        "transport-eof-reaps-setsid-descendants",
        stopped(path) and progress.read_text() == before,
    )
    marker = a.evidence / "registered-no-frame-must-not-run"
    ident = str(uuid.uuid4())
    path = a.evidence / (ident + ".json")
    env = {"PATH": "/usr/bin:/bin", "POCKYMOE_HOST_PROCESS_RECEIPT": str(path)}
    path.write_text(
        json.dumps(
            {
                "threadId": "guardian-fixture",
                "id": ident,
                "pid": None,
                "start": None,
                "stopped": False,
            }
        )
    )
    c = subprocess.Popen(
        [str(a.binary.resolve()), "environment-worker", "host-run", ident],
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    path.write_text(
        json.dumps(
            {
                "threadId": "guardian-fixture",
                "id": ident,
                "pid": c.pid,
                "start": start_tick(c.pid),
                "stopped": False,
            }
        )
    )
    c.stdin.close()
    c.wait(timeout=5)
    check(
        "registered-without-launch-frame-proves-no-child-executed",
        not marker.exists() and stopped(path),
    )
    # A SIGKILL guardian cannot sign an exit proof; a missing PID is deliberately
    # not converted into stopped=true by the controller.
    c, path = launch(True, ["/bin/sleep", ".3"])
    c.kill()
    c.wait(timeout=5)
    check("guardian-sigkill-does-not-forge-quiescence", not stopped(path))
    c.stdin.close()


if __name__ == "__main__":
    main()
