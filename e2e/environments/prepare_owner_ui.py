import json, sys, shutil
from pathlib import Path
from native_credentials import attach
from reincarnation import HERE, run, wait

r = Path(sys.argv[1])
x = attach(r)
p = x.root / "workspaces" / ("owner-browser-" + __import__("secrets").token_hex(3))
p.mkdir()
(p / ".fixture").mkdir()
(p / "preserved.txt").write_text("keep this owner file untouched\n")
shutil.copy2(HERE / "reincarnation_agent.py", p / ".fixture/handoff.py")
run(["git", "init", p])
run(["git", "-C", p, "add", "."])
run(
    [
        "git",
        "-C",
        p,
        "-c",
        "user.name=Owner browser fixture",
        "-c",
        "user.email=fixture@test.invalid",
        "commit",
        "-m",
        "owner browser baseline",
    ]
)
w = x.api("/api/workspaces", {"absPath": str(p), "label": "Real owner browser"})
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
(p / ".fixture/spec.json").write_text(
    json.dumps(
        {
            "connectionId": x.manifest["connectionId"],
            "name": "Real owner browser proposal",
            "limits": {
                "cpu": 0.25,
                "memoryBytes": 768 * 1024**2,
                "pids": 128,
                "nofile": 512,
            },
            "authentication": {"codex": "apiBroker", "claude": "apiBroker"},
            "egressHosts": [],
            "credentialMounts": [],
            "handoff": "Owner constraint: never modify preserved.txt.",
        }
    )
)
x.api(
    "/api/threads/" + t["id"] + "/prompt",
    {"prompt": "fixture: propose. Owner constraint: never modify preserved.txt."},
)
v = wait(
    lambda: (
        json.loads((p / "proposal-receipt.json").read_text())
        if (p / "proposal-receipt.json").exists()
        else None
    )
)
(x.root / "owner-ui-input.json").write_text(
    json.dumps(
        {
            "threadId": t["id"],
            "requestId": v["id"],
            "digest": v["digest"],
            "project": str(p),
        }
    )
)
print("Actual CLI proposal pending; owner browser input prepared")
