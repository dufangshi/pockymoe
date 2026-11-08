#!/usr/bin/env python3
"""Real owner requests/restart/failure recovery in a retained independent lab."""
import argparse, json, shutil, time, urllib.request
from pathlib import Path
from native_credentials import attach
from reincarnation import HERE, run, wait


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--run-manifest", type=Path, required=True)
    a = p.parse_args()
    x = attach(a.run_manifest)
    area = x.root / "request-boundaries"
    area.mkdir()
    e = []

    def check(name, value, details=None):
        assert value, "Boundary failed: " + name
        e.append({"name": name, "passed": True, "details": details or {}})
        (area / "evidence.json").write_text(json.dumps(e, indent=2) + "\n")
        print("PASS", name, flush=True)

    def new(name):
        project = x.root / "workspaces" / name
        project.mkdir()
        (project / ".fixture").mkdir()
        (project / "preserved.txt").write_text("keep this owner file untouched\n")
        shutil.copy2(HERE / "reincarnation_agent.py", project / ".fixture/handoff.py")
        run(["git", "init", project])
        run(["git", "-C", project, "add", "."])
        run(
            [
                "git",
                "-C",
                project,
                "-c",
                "user.name=Boundary fixture",
                "-c",
                "user.email=fixture@test.invalid",
                "commit",
                "-m",
                "boundary baseline",
            ]
        )
        w = x.api("/api/workspaces", {"absPath": str(project), "label": name})
        t = x.api(
            "/api/threads/start",
            {
                "workspaceId": w["id"],
                "provider": "acp",
                "agentId": "custom",
                "model": "fixture",
                "reasoningEffort": "max",
                "approvalMode": "yolo",
            },
        )
        spec = {
            "connectionId": x.manifest["connectionId"],
            "name": name,
            "limits": {
                "cpu": 0.25,
                "memoryBytes": 768 * 1024**2,
                "pids": 128,
                "nofile": 512,
            },
            "authentication": {"codex": "apiBroker", "claude": "apiBroker"},
            "egressHosts": [],
            "credentialMounts": [],
            "handoff": "Owner constraint: never modify preserved.txt. Inspect state and never replay commands.",
        }
        (project / ".fixture/spec.json").write_text(json.dumps(spec))
        return project, t

    def propose(project, t):
        x.api(
            "/api/threads/" + t["id"] + "/prompt",
            {
                "prompt": "fixture: propose. Owner constraint: never modify preserved.txt."
            },
        )
        r = wait(
            lambda: (
                json.loads((project / "proposal-receipt.json").read_text())
                if (project / "proposal-receipt.json").exists()
                else None
            )
        )
        wait(
            lambda: (project / "agent-request-security.json").exists()
            and (project / "callback-writer-count").exists()
        )
        return r

    def decide(r, action):
        return x.api(
            "/api/environment-requests/" + r["id"] + "/" + action,
            {"digest": r["digest"]},
        )

    def finished(t):
        wait(lambda: x.api("/api/threads/" + t["id"])["thread"]["status"] == "idle")

    def restart():
        before = json.loads((x.root / "private/host-process.json").read_text())
        run(
            x.docker
            + ["exec", x.container, "kill", "-KILL", str(before["supervisorPid"])]
        )
        wait(
            lambda: json.loads((x.root / "private/host-process.json").read_text())[
                "restartCount"
            ]
            > before["restartCount"]
        )
        wait(
            lambda: urllib.request.urlopen(x.base + "/healthz", timeout=2).status == 200
        )
        return before

    # Pending metadata survives a real Supervisor SIGKILL. The guardian continues
    # long enough to reap every pre-crash writer and sign its durable exit proof.
    project, t = new("restart-approved")
    r = propose(project, t)
    restart()
    wait(lambda: x.api("/api/environment-requests/" + r["id"])["state"] == "pending")
    counts = [project / n for n in ["host-writer-count", "callback-writer-count"]]
    last = [p.read_bytes() for p in counts]
    time.sleep(0.15)
    check(
        "supervisor-sigkill-preserves-pending-and-stops-host-native-callback-writers",
        last == [p.read_bytes() for p in counts],
    )
    decide(r, "approve")
    ready = wait(
        lambda: (
            v
            if (v := x.api("/api/environment-requests/" + r["id"]))["state"]
            in ["ready", "failed"]
            else None
        ),
        110,
    )
    check(
        "approved-request-recovers-after-real-supervisor-restart",
        ready["state"] == "ready",
        {"requestState": ready["state"], "lastError": ready.get("lastError")},
    )
    result = wait(
        lambda: (
            json.loads((project / "continuation-evidence.json").read_text())
            if (project / "continuation-evidence.json").exists()
            else None
        )
    )
    check(
        "crashed-turn-never-replayed-one-fresh-continuation",
        result["continuationCount"] == 1 and result["sameThread"],
    )
    finished(t)
    restart()
    time.sleep(1)
    check(
        "ready-restart-does-not-queue-another-continuation",
        int((project / "continuation-count").read_text()) == 1,
    )
    x.api("/api/environments/" + ready["environmentId"] + "/destroy", {})
    # Reject and caller cancellation grant no computation and preserve Host mode.
    project, t = new("rejected")
    r = propose(project, t)
    v = decide(r, "reject")
    (project / "allow-host-finish").touch()
    finished(t)
    check(
        "owner-rejection-no-environment-or-migration",
        v["state"] == "rejected"
        and not x.api("/api/environment-requests/" + r["id"]).get("environmentId")
        and not (project / "continuation-count").exists(),
    )
    project, t = new("agent-cancelled")
    r = propose(project, t)
    (project / "cancel-request").touch()
    v = wait(
        lambda: (
            json.loads((project / "cancel-receipt.json").read_text())
            if (project / "cancel-receipt.json").exists()
            else None
        )
    )
    (project / "allow-host-finish").touch()
    finished(t)
    check(
        "authenticated-agent-can-cancel-before-host-fence",
        v["state"] == "cancelled" and not v.get("environmentId"),
    )
    # A changed reviewed connection causes an actionable fenced failure; restoring
    # it permits retry. Repeating owner approval/retry never duplicates deployment.
    project, t = new("changed-connection")
    r = propose(project, t)
    decide(r, "approve")
    cfg = json.loads(Path(x.lab["connectionFile"]).read_text())
    cfg.update(
        baseUrl="http://" + x.lab["dokployContainer"] + ":3000",
        image=x.lab["nativeImage"],
        allowedRoots=[str(x.root / "workspaces"), str(x.root / "external")],
        cpuLimit=0.5,
        memoryLimitBytes=2147483648,
        pidsLimit=256,
        nofileLimit=1024,
    )
    original = dict(cfg)
    cfg["name"] = "Changed reviewed policy"
    x.api("/api/environments/connections/" + x.manifest["connectionId"], cfg, "PUT")
    (project / "allow-host-finish").touch()
    v = wait(
        lambda: (
            v
            if (v := x.api("/api/environment-requests/" + r["id"]))["state"] == "failed"
            else None
        )
    )
    check(
        "connection-edit-fails-approved-request-without-host-fallback",
        v["state"] == "failed" and not v.get("environmentId"),
    )
    x.api(
        "/api/threads/" + t["id"] + "/prompt",
        {"prompt": "must be denied while fenced"},
        expected=409,
    )
    original["name"] = r["spec"]["connection"]["name"]
    x.api(
        "/api/environments/connections/" + x.manifest["connectionId"], original, "PUT"
    )
    decide(r, "retry")
    decide(r, "retry")
    ready = wait(
        lambda: (
            v
            if (v := x.api("/api/environment-requests/" + r["id"]))["state"]
            in ["ready", "failed"]
            else None
        ),
        110,
    )
    check(
        "owner-retry-same-approved-spec-provisions-once",
        ready["state"] == "ready",
        {"state": ready["state"], "lastError": ready.get("lastError")},
    )
    wait(lambda: (project / "continuation-evidence.json").exists())
    finished(t)
    x.api("/api/environments/" + ready["environmentId"] + "/destroy", {})
    # A source/settings failure is recoverable only via a fresh owner decision;
    # this action explicitly abandons isolation and retains original project/state.
    project, t = new("explicit-host-recovery")
    r = propose(project, t)
    decide(r, "approve")
    cfg["name"] = "Policy changed again"
    x.api("/api/environments/connections/" + x.manifest["connectionId"], cfg, "PUT")
    (project / "allow-host-finish").touch()
    wait(lambda: x.api("/api/environment-requests/" + r["id"])["state"] == "failed")
    v = decide(r, "recoverHost")
    again = decide(r, "recoverHost")
    check(
        "explicit-owner-host-recovery-idempotent-and-project-preserved",
        v["state"] == again["state"] == "cancelled"
        and (project / "preserved.txt").exists(),
    )
    x.api(
        "/api/threads/" + t["id"] + "/prompt",
        {"prompt": "Host recovery explicitly authorized; report ready."},
    )
    finished(t)
    check(
        "host-execution-resumes-only-after-explicit-owner-recovery",
        not (project / "continuation-count").exists(),
    )
    x.api(
        "/api/environments/connections/" + x.manifest["connectionId"], original, "PUT"
    )
    (area / "manifest.json").write_text(
        json.dumps(
            {
                "runManifest": str(a.run_manifest),
                "status": "passed",
                "realSupervisorSigkill": True,
                "realDokploy": True,
            },
            indent=2,
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
