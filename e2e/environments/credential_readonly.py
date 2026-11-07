#!/usr/bin/env python3
"""Real native single-file RO bind, inode replacement and no parent widening."""
import argparse, json, shutil, os
from pathlib import Path
from native_credentials import attach
from reincarnation import HERE, run, wait


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--run-manifest", type=Path, required=True)
    a = p.parse_args()
    x = attach(a.run_manifest)
    meta = json.loads((x.root / "native-auth/manifest.json").read_text())
    x.api("/api/environments/" + meta["environmentId"] + "/destroy", {})
    area = x.root / "credential-readonly"
    area.mkdir()
    e = []

    def check(name, value):
        assert value, "RO credential regression failed: " + name
        e.append({"name": name, "passed": True})
        (area / "evidence.json").write_text(json.dumps(e, indent=2) + "\n")
        print("PASS", name, flush=True)

    source = Path(meta["sources"]) / "codex/auth.json"
    original = source.read_bytes()
    parent_canary = source.parent / "unauthorized-parent-canary"
    parent_canary.write_text("public metadata canary; parent is not granted\n")
    project = x.root / "workspaces/credential-readonly"
    project.mkdir()
    (project / ".fixture").mkdir()
    (project / "preserved.txt").write_text("keep this owner file untouched\n")
    shutil.copy2(HERE / "reincarnation_agent.py", project / ".fixture/handoff.py")
    probe = """import json,os,subprocess,uuid
from pathlib import Path
p=Path('/pockymoe/state/codex/auth.json');v={}
try:
 with p.open('ab') as f:f.write(b'\\n')
 v['readOnlyNativeAuthEnforced']=False
except OSError:v['readOnlyNativeAuthEnforced']=True
v['sourceParentNotGranted']=not Path('/pockymoe/state/codex/unauthorized-parent-canary').exists()
# Keep stdin open through the actual child, as ACP does. The real CLI attempts
# its own refresh; no OAuth token is put into an API profile or exported env.
env={k:v for k,v in os.environ.items() if k in ['PATH','HOME','CODEX_HOME','HTTPS_PROXY','NO_PROXY','SSL_CERT_FILE']};env['POCKYMOE_ENV_CODEX_NATIVE']='1';ident=str(uuid.uuid4());req={'id':ident,'argv':['/opt/pockymoe/bin/codex-subscription','app-server'],'cwd':os.getcwd(),'env':list(env.items())};c=subprocess.Popen(['/opt/pockymoe/bin/pockymoe','environment-worker','run',ident],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
c.stdin.write((json.dumps(req)+'\\n'+json.dumps({'id':1,'method':'initialize','params':{'clientInfo':{'name':'synthetic_ro','version':'1'}}})+'\\n').encode());c.stdin.flush()
while json.loads(c.stdout.readline()).get('id')!=1:pass
c.stdin.write((json.dumps({'method':'initialized'})+'\\n'+json.dumps({'id':2,'method':'account/read','params':{'refreshToken':True}})+'\\n').encode());c.stdin.flush()
while True:
 r=json.loads(c.stdout.readline())
 if r.get('id')==2:break
v['nativeRefreshReadOnlyExplicitFailure']='error' in r
c.stdin.close();c.wait(timeout=8);Path('readonly-credential-evidence.json').write_text(json.dumps(v)+'\\n')
"""
    (project / ".fixture/native_validation.py").write_text(probe)
    run(["git", "init", project])
    run(["git", "-C", project, "add", "."])
    run(
        [
            "git",
            "-C",
            project,
            "-c",
            "user.name=RO fixture",
            "-c",
            "user.email=fixture@test.invalid",
            "commit",
            "-m",
            "RO fixture baseline",
        ]
    )
    w = x.api(
        "/api/workspaces", {"absPath": str(project), "label": "Native single file RO"}
    )
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
        "name": "Read-only native auth file",
        "limits": {"cpu": 0.25, "memoryBytes": 1024**3, "pids": 128, "nofile": 1024},
        "authentication": {"codex": "nativeFile", "claude": "apiBroker"},
        "egressHosts": ["auth.openai.com", "chatgpt.com"],
        "credentialMounts": [
            {
                "source": str(source),
                "target": "/pockymoe/state/codex/auth.json",
                "readOnly": True,
                "purpose": "codex",
                "environment": {"CODEX_HOME": "/pockymoe/state/codex"},
            },
            {
                "source": str(x.root / "native-auth/private/ca.crt"),
                "target": "/credentials/fixture-ca.crt",
                "readOnly": True,
                "purpose": "generic",
                "environment": {"SSL_CERT_FILE": "/credentials/fixture-ca.crt"},
            },
        ],
        "handoff": "Synthetic RO file test. Owner constraint: never modify preserved.txt.",
    }
    (project / ".fixture/spec.json").write_text(json.dumps(spec))
    x.api(
        "/api/threads/" + t["id"] + "/prompt",
        {
            "prompt": "fixture: propose RO file. Owner constraint: never modify preserved.txt."
        },
    )
    r = wait(
        lambda: (
            json.loads((project / "proposal-receipt.json").read_text())
            if (project / "proposal-receipt.json").exists()
            else None
        )
    )
    x.api("/api/environment-requests/" + r["id"] + "/approve", {"digest": r["digest"]})
    (project / "allow-host-finish").touch()
    ready = wait(
        lambda: (
            v
            if (v := x.api("/api/environment-requests/" + r["id"]))["state"]
            in ["ready", "failed"]
            else None
        ),
        110,
    )
    check("native-single-file-grant-deployed", ready["state"] == "ready")
    proof = wait(
        lambda: (
            json.loads((project / "readonly-credential-evidence.json").read_text())
            if (project / "readonly-credential-evidence.json").exists()
            else None
        ),
        100,
    )
    for name, passed in proof.items():
        check(name, passed)
    check(
        "read-only-refresh-never-modifies-original-auth",
        source.read_bytes() == original,
    )
    environment = next(
        e
        for e in x.api("/api/environments")["environments"]
        if e["id"] == ready["environmentId"]
    )
    generation = environment["generation"]
    replacement = source.with_name("replacement.auth.json")
    replacement.write_bytes(original)
    replacement.chmod(0o600)
    os.replace(replacement, source)
    changed = wait(
        lambda: (
            v
            if (
                v := next(
                    e
                    for e in x.api("/api/environments")["environments"]
                    if e["id"] == ready["environmentId"]
                )
            )["generation"]
            > generation
            and v["state"] in ["interrupted", "error", "stopped"]
            else None
        ),
        25,
    )
    check(
        "host-atomic-file-replacement-fences-generation",
        changed["generation"] > generation,
    )
    (area / "manifest.json").write_text(
        json.dumps(
            {
                "threadId": t["id"],
                "requestId": r["id"],
                "environmentId": ready["environmentId"],
                "project": str(project),
                "oldGeneration": generation,
                "fencedGeneration": changed["generation"],
            },
            indent=2,
        )
        + "\n"
    )
    x.api("/api/environments/" + ready["environmentId"] + "/destroy", {})
    check(
        "destroy-preserves-original-auth-and-project",
        source.exists() and (project / "preserved.txt").exists(),
    )


if __name__ == "__main__":
    main()
