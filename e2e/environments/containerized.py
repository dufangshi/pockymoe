#!/usr/bin/env python3
"""Real namespace translation through a lab-only containerized Supervisor."""
import argparse
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import sys
import tarfile
import time
import uuid

sys.dont_write_bytecode = True
from verify import (Evidence, Failure, HERE, REPOSITORY, basic_env, docker_command,
                    git, load_connection, load_state, private_write, process,
                    require, sha256_file, write_json)


# Executed only in our control-plane container. Its private CLI token never
# leaves the container; the driver receives just the requested HTTP response.
HTTP_CLIENT = r'''
import json,sys,urllib.request,urllib.error
from pathlib import Path
v=json.load(sys.stdin)
h={"Content-Type":"application/json"}
if v["path"]=="/api/cli":
    h["Authorization"]="Bearer "+json.loads(Path("/fixture/state/supervisor.cli.json").read_text())["token"]
r=urllib.request.Request("http://127.0.0.1:45679"+v["path"],headers=h,
    data=None if v["body"] is None else json.dumps(v["body"]).encode(),method=v["method"])
try:
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(r,timeout=100) as response:
        status,data=response.status,response.read(4*1024*1024)
except urllib.error.HTTPError as e:
    status,data=e.code,e.read(1024*1024)
try: value=json.loads(data) if data else None
except ValueError: value=None
print(json.dumps({"status":status,"value":value}))
'''


class ContainerAPI:
    def __init__(self, connection, container, evidence):
        self.connection, self.container, self.evidence = connection, container, evidence

    def request(self, path, body=None, method=None, allowed=(200,)):
        result = docker_command(self.connection, "exec", "-i", self.container,
            "python3", "-c", HTTP_CLIENT, input=json.dumps({"path": path, "body": body,
                "method": method or ("GET" if body is None else "POST")}).encode(), timeout=115)
        response = json.loads(result.stdout)
        self.evidence.event(method or ("GET" if body is None else "POST"), path, response["status"])
        if response["status"] not in allowed:
            with (self.evidence.root / "http-errors.jsonl").open("a") as file:
                file.write(json.dumps({"path": path, "status": response["status"], "knownErrorClasses":
                    [message for message in ["Docker bind identity probe failed", "container layer", "not shared storage",
                        "remote storage requires an explicit adapter"] if rejection(response["value"], [message])]}) + "\n")
        require(response["status"] in allowed, "Container Supervisor HTTP returned " + str(response["status"]) + " for " + path)
        return response["status"], response["value"]

    def __call__(self, path, body=None, method=None):
        return self.request(path, body, method)[1]


def project(path):
    path.mkdir(parents=True)
    (path / ".fixture").mkdir()
    for name in ["acp_agent.py", "fixture_service.py"]:
        shutil.copyfile(HERE / name, path / ".fixture" / name)
    (path / "tracked.txt").write_text("namespace-fixture-baseline\n")
    git(path, "init", "-q", "-b", "main")
    git(path, "add", ".")
    git(path, "commit", "-qm", "Namespace fixture baseline")


def build_image(connection, image, base_image, overlay, uid, gid, root):
    # The context contains one public Dockerfile. No private scratch is sent.
    instructions = (f"FROM {base_image}\nUSER 0\n"
        f"RUN mkdir -p {overlay}/.fixture && cp /opt/pockymoe-test/acp_agent.py /opt/pockymoe-test/fixture_service.py {overlay}/.fixture/ "
        f"&& cd {overlay} && git init -q -b main "
        "&& printf 'image-layer-fixture\\n' > tracked.txt && git add tracked.txt "
        "&& git -c user.name=Fixture -c user.email=fixture@test.invalid commit -qm baseline "
        f"&& chown -R {uid}:{gid} {overlay}\n")
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as stream:
        entry = tarfile.TarInfo("Dockerfile")
        data = instructions.encode()
        entry.size = len(data)
        stream.addfile(entry, io.BytesIO(data))
    result = docker_command(connection, "build", "--memory", "256m", "--cpu-period", "100000",
        "--cpu-quota", "50000", "--tag", image, "-", input=archive.getvalue(), timeout=300)
    (root / "image-build.log").write_bytes(result.stdout + result.stderr)
    inspected = json.loads(docker_command(connection, "image", "inspect", image).stdout)[0]
    write_json(root / "image.json", {"image": image, "id": inspected["Id"], "size": inspected["Size"]})


def new_thread(api, path, prefix, label):
    workspace = api("/api/workspaces", {"absPath": path, "label": prefix + "-" + label})
    return api("/api/threads/start", {"workspaceId": workspace["id"], "provider": "acp", "agentId": "custom",
        "model": "fixture", "title": prefix + "-" + label, "approvalMode": "yolo"})


def rejection(value, expected):
    # Never serialize arbitrary error messages or response bodies.
    text = json.dumps(value)
    return any(message in text for message in expected)


def run(args):
    state, parent = load_state(args.manifest)
    connection = load_connection(Path(state["connectionFile"]))
    require(args.binary is not None or state.get("binary"), "Complete the main run or provide a newly approved binary explicitly")
    approved_binary = (args.binary or Path(state["binary"])).resolve(strict=True)
    require(approved_binary.is_relative_to(REPOSITORY), "Binary must be visible to the dedicated lab daemon")
    binary_sha256 = sha256_file(approved_binary)
    if args.binary is None:
        require(approved_binary.is_relative_to(parent / "bin") and binary_sha256 == state["binarySha256"], "Approved pinned binary changed")
    commit = process(["git", "rev-parse", args.ready_commit + "^{commit}"], cwd=REPOSITORY).stdout.decode().strip()
    if args.binary is None:
        require(commit == state.get("readyCommit"), "A new ready commit requires an explicitly supplied new binary")
    process(["git", "merge-base", "--is-ancestor", commit, "HEAD"], cwd=REPOSITORY)
    prefix = state["prefix"] + "-namespace-" + secrets.token_hex(3)
    root = parent / ("containerized-" + prefix.rsplit("-", 1)[1])
    root.mkdir(mode=0o700)
    for name in ["private", "home", "state", "workspaces", "bin"]:
        (root / name).mkdir(mode=0o700)
    # Preserve the earlier main-run pin and never overwrite a live file bind.
    binary = root / "bin" / ("pockymoe-" + binary_sha256[:20])
    shutil.copyfile(approved_binary, binary)
    binary.chmod(0o755)
    require(sha256_file(binary) == binary_sha256, "Ready binary changed while pinning")
    evidence = Evidence(root, [connection["apiKey"]])
    source = root / "workspaces/project"
    overlay = root / "overlay-project"
    project(source)
    project(overlay)
    # The daemon really has this same absolute path. The Supervisor image has
    # different contents at that path and no bind mount covering it.
    (overlay / "tracked.txt").write_text("daemon-host-fixture\n")
    baseline = (overlay / "tracked.txt").read_bytes()
    docker_cli = root / "bin/docker"
    shutil.copyfile(shutil.which("docker"), docker_cli)
    docker_cli.chmod(0o755)
    image = prefix + ":supervisor"
    container = prefix + "-supervisor"
    resources = {"prefix": prefix, "readyCommit": commit, "binarySha256": binary_sha256, "binary": str(binary),
        "supervisorContainer": container, "image": image, "hostProjectPath": str(source),
        "supervisorProjectPath": "/fixture/workspaces/project", "imageLayerPath": str(overlay),
        "databasePath": str(root / "state/supervisor.sqlite"), "environmentIds": []}
    write_json(root / "resources.json", resources)
    api = None
    try:
        loader = docker_command(connection, "run", "--rm", "--name", prefix + "-ldd", "--network", "none",
            "--user", f"{os.getuid()}:{os.getgid()}", "--read-only", "--cpus", ".25", "--memory", "128m",
            "--pids-limit", "32", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
            "--mount", f"type=bind,source={binary},target=/opt/pockymoe/bin/pockymoe,readonly",
            "--entrypoint", "ldd", state["image"], "/opt/pockymoe/bin/pockymoe", checked=False)
        loader_output = (loader.stdout + loader.stderr).decode(errors="replace")
        require(evidence.check("namespace-approved-binary-actual-loader-compatible", loader.returncode == 0
            and "not found" not in loader_output and "version `" not in loader_output), "Namespace binary loader incompatible")
        write_json(root / "binary-compatibility.json", {"binary": str(binary), "sha256": binary_sha256, "ldd": loader_output})
        build_image(connection, image, state["image"], overlay, os.getuid(), os.getgid(), root)
        socket_path = Path(connection["dockerHost"].removeprefix("unix://"))
        network = connection["name"] + "-backend"
        networks = json.loads(docker_command(connection, "network", "inspect", network).stdout)
        require(len(networks) == 1 and networks[0]["Name"] == network, "Dedicated backend network unavailable")
        argv = ["run", "-d", "--name", container, "--label", "pockymoe.acceptance=" + prefix,
            "--network", network, "--user", f"{os.getuid()}:{os.getgid()}", "--group-add", str(socket_path.stat().st_gid),
            "--read-only", "--tmpfs", "/tmp:rw,nosuid,nodev,size=64m", "--tmpfs", "/run:rw,nosuid,nodev,size=4m",
            "--tmpfs", str(overlay) + f".worktrees:rw,nosuid,nodev,size=4m,uid={os.getuid()},gid={os.getgid()},mode=700",
            "--cpus", ".25", "--memory", "256m", "--memory-swap", "256m", "--pids-limit", "96", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges:true", "--ulimit", "nofile=1024:1024",
            "--log-opt", "max-size=1m", "--log-opt", "max-file=1", "--workdir", "/fixture/workspaces/project"]
        for host, target, readonly in [(root / "workspaces", "/fixture/workspaces", False),
                (root / "state", "/fixture/state", False), (root / "home", "/fixture/home", False),
                (binary, "/opt/pockymoe/bin/pockymoe", True), (docker_cli, "/usr/bin/docker", True),
                (socket_path, "/fixture/docker.sock", True)]:
            argv.extend(["--mount", f"type=bind,source={host},target={target}" + (",readonly" if readonly else "")])
        environment = basic_env(Path("/fixture/home"))
        environment.update({"POCKYMOE_DATABASE_PATH": "/fixture/state/supervisor.sqlite",
            "POCKYMOE_WORKSPACE_ROOT": "/fixture/workspaces", "POCKYMOE_MODE": "local",
            "POCKYMOE_HOST_MODE": "hosted", "POCKYMOE_HOST": "127.0.0.1", "POCKYMOE_PORT": "45679",
            "POCKYMOE_ENABLED_AGENT_PROVIDERS": "acp", "POCKYMOE_ACP_COMMAND": "python3 .fixture/acp_agent.py",
            "RUST_LOG": "warn", "POCKYMOE_ENVIRONMENT": "test"})
        for name, value in environment.items():
            argv.extend(["--env", name + "=" + value])
        argv.extend(["--entrypoint", "/opt/pockymoe/bin/pockymoe", image, "supervisor"])
        identifier = docker_command(connection, *argv).stdout.decode().strip()
        resources["supervisorContainerId"] = identifier
        write_json(root / "resources.json", resources)
        api = ContainerAPI(connection, container, evidence)
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            try:
                api("/healthz")
                api("/api/environments")
                break
            except (Failure, ValueError):
                time.sleep(0.2)
        else:
            raise Failure("Containerized Supervisor did not become ready")
        inspect = json.loads(docker_command(connection, "inspect", identifier).stdout)[0]
        evidence.check("real-containerized-supervisor-private-identity-and-limits",
            inspect["State"]["Running"] and inspect["HostConfig"]["Memory"] == 268435456
            and inspect["HostConfig"]["NanoCpus"] == 250000000 and not inspect["HostConfig"]["Privileged"]
            and not inspect["HostConfig"].get("PortBindings"), {"containerId": identifier, "databasePath": resources["databasePath"]})
        cli_check = docker_command(connection, "exec", container, "sh", "-c", "ldd /usr/bin/docker && /usr/bin/docker --version")
        evidence.check("docker-cli-actually-loads-inside-supervisor-image", cli_check.returncode == 0)
        request = dict(connection, name=prefix, baseUrl="http://" + connection["name"] + "-dokploy:3000",
            dockerHost="unix:///fixture/docker.sock", image=state["image"],
            allowedRoots=["/fixture/workspaces", str(overlay)], cpuLimit=.25,
            memoryLimitBytes=268435456, pidsLimit=96, nofileLimit=1024)
        configured = api("/api/environments/connections/new", request, "PUT")
        evidence.check("container-connection-api-key-write-only", "apiKey" not in configured and connection["apiKey"] not in json.dumps(configured))
        cid = configured["id"]
        resources["connectionId"] = cid
        tested = api(f"/api/environments/connections/{cid}/test", {})
        require(evidence.check("container-real-dokploy-api-connectivity", tested.get("ok") is True), "Container Dokploy API unavailable")
        thread = new_thread(api, "/fixture/workspaces/project", prefix, "mapped")
        worker = api("/api/environments", {"connectionId": cid, "threadId": thread["id"], "name": prefix + "-mapped"})
        resources["environmentIds"].append(worker["id"])
        resources["threadId"] = thread["id"]
        write_json(root / "resources.json", resources)
        worker = api(f"/api/environments/{worker['id']}/start", {})
        require(evidence.check("container-supervisor-real-provider-worker-starts", worker.get("state") == "running"), "Mapped worker did not start")
        api(f"/api/threads/{thread['id']}/environment", {"environmentId": worker["id"], "resetSession": True}, "PUT")
        actual = json.loads(docker_command(connection, "inspect", worker["containerId"]).stdout)[0]
        mapped = [m for m in actual["Mounts"] if m["Destination"] == "/fixture/workspaces/project"]
        evidence.check("container-path-translated-to-real-daemon-bind-source",
            len(mapped) == 1 and mapped[0]["Source"] == str(source) and mapped[0]["RW"],
            {"source": mapped[0]["Source"] if mapped else None, "target": "/fixture/workspaces/project"})
        marker = "namespace-written-" + uuid.uuid4().hex + ".txt"
        code = "import json,os;from pathlib import Path;p=Path('.');s=p.stat();Path(" + repr(marker) + ").write_text('worker-write\\n');print(json.dumps({'device':s.st_dev,'inode':s.st_ino,'workerMarked':os.environ.get('POCKYMOE_ENVIRONMENT_WORKER')=='1'}))"
        result = api("/api/cli", {"operation": "commandRun", "threadId": thread["id"], "input": {"argv": ["python3", "-c", code], "cwd": ".", "timeoutSeconds": 15}})
        info = json.loads(result.get("stdout") or "{}")
        host_stat = source.stat()
        evidence.check("container-supervisor-command-uses-actual-host-storage",
            result.get("state") == "completed" and result.get("exitCode") == 0 and (source / marker).exists()
            and info.get("device") == host_stat.st_dev and info.get("inode") == host_stat.st_ino and info.get("workerMarked") is True,
            {"deviceMatches": info.get("device") == host_stat.st_dev, "inodeMatches": info.get("inode") == host_stat.st_ino,
             "hostWriteVisible": (source / marker).exists(), "commandState": result.get("state")})
        evidence.check("worker-does-not-inherit-supervisor-control-socket-or-database",
            not any(m["Destination"] in {"/fixture/docker.sock", "/fixture/state", "/fixture/home"} for m in actual["Mounts"])
            and connection["apiKey"] not in "\n".join(actual["Config"].get("Env", [])))
        api(f"/api/environments/{worker['id']}/stop", {})
        forbidden = "namespace-fallback-" + uuid.uuid4().hex
        require(not (source / forbidden).exists(), "Fresh namespace fallback marker exists")
        status, refused = api.request("/api/cli", {"operation": "commandRun", "threadId": thread["id"],
            "input": {"argv": ["python3", "-c", "from pathlib import Path;Path(" + repr("/fixture/workspaces/project/" + forbidden) + ").write_text('unexpected')"],
                "cwd": ".", "timeoutSeconds": 5}}, allowed=(200, 400, 409, 500, 503))
        failed = (isinstance(refused, dict) and refused.get("state") == "failed" and refused.get("exitCode") is None
            and not refused.get("stdout") and not refused.get("stderr") and "environment_not_running" in str(refused.get("error", "")))
        evidence.check("container-stopped-command-refused-without-host-fallback",
            (400 <= status < 500 or (status == 200 and failed)) and not (source / forbidden).exists(),
            {"httpStatus": status, "commandState": refused.get("state") if isinstance(refused, dict) else None,
             "environmentNotRunning": failed, "preexistingBefore": False, "existsAfter": (source / forbidden).exists(), "mtimeAfter": None})

        # Prove the negative is a real image layer with an existing daemon-side
        # same-name project, then require the real API to reject source mapping.
        layer = docker_command(connection, "exec", container, "python3", "-c",
            "import json;from pathlib import Path;p=Path(" + repr(str(overlay)) + ");s=p.stat();print(json.dumps({'device':s.st_dev,'inode':s.st_ino,'imageLayerMarker':p.joinpath('tracked.txt').read_text()=='image-layer-fixture\\n'}))")
        layer_info = json.loads(layer.stdout)
        overlay_stat = overlay.stat()
        evidence.check("same-path-project-is-actual-unshared-image-layer",
            layer_info["imageLayerMarker"] and (overlay_stat.st_dev, overlay_stat.st_ino) != (layer_info["device"], layer_info["inode"])
            and not any(Path(str(overlay)).is_relative_to(m["Destination"]) for m in inspect["Mounts"]),
            {"daemonSamePathExists": overlay.is_dir(), "differentDeviceOrInode": (overlay_stat.st_dev, overlay_stat.st_ino) != (layer_info["device"], layer_info["inode"])})
        layer_thread = new_thread(api, str(overlay), prefix, "layer")
        layer_worker = api("/api/environments", {"connectionId": cid, "threadId": layer_thread["id"], "name": prefix + "-layer"})
        resources["environmentIds"].append(layer_worker["id"])
        write_json(root / "resources.json", resources)
        status, denied = api.request(f"/api/environments/{layer_worker['id']}/start", {}, allowed=(200, 400, 409, 500))
        layer_state = next(item for item in api("/api/environments")["environments"] if item["id"] == layer_worker["id"])
        evidence.check("same-path-image-layer-source-rejected-before-deployment",
            400 <= status < 500 and rejection(denied, ["container layer", "not shared storage"])
            and layer_state.get("state") != "running" and not layer_state.get("containerId") and not layer_state.get("composeId")
            and (overlay / "tracked.txt").read_bytes() == baseline,
            {"httpStatus": status, "explicitContainerLayerRejection": rejection(denied, ["container layer", "not shared storage"]),
             "environmentState": layer_state.get("state"), "composeCreated": bool(layer_state.get("composeId")), "hostSamePathUnchanged": (overlay / "tracked.txt").read_bytes() == baseline})

        remote_request = dict(request, dockerHost="tcp://127.0.0.1:2375", name=prefix + "-remote")
        status, denied = api.request("/api/environments/connections/new", remote_request, "PUT", allowed=(200, 400, 409, 500))
        evidence.check("unknown-remote-docker-rejected-at-configuration",
            400 <= status < 500 and rejection(denied, ["only a local Unix Docker socket", "remote storage requires an explicit adapter"]),
            {"httpStatus": status, "explicitUnsupportedRemote": rejection(denied, ["remote storage requires an explicit adapter"]),
             "scope": "configuration validation; no remote daemon mutation"})
        for environment_id in resources["environmentIds"]:
            destroyed = api(f"/api/environments/{environment_id}/destroy", {})
            evidence.check("namespace-owned-environment-destroyed-" + environment_id, destroyed.get("state") == "destroyed")
        evidence.check("namespace-cleanup-preserves-original-git-and-same-path-project",
            (source / marker).exists() and (overlay / "tracked.txt").read_bytes() == baseline and len(git(source, "rev-parse", "HEAD")) == 40)
    except Exception as error:
        evidence.check("containerized-acceptance-execution", False, evidence.redact(str(error)) if isinstance(error, Failure) else type(error).__name__)
    finally:
        if resources.get("supervisorContainerId"):
            identifier = resources["supervisorContainerId"]
            own = docker_command(connection, "inspect", identifier, checked=False)
            if own.returncode == 0:
                inspected = json.loads(own.stdout)[0]
                require(inspected["Config"]["Labels"].get("pockymoe.acceptance") == prefix, "Namespace cleanup ownership differs")
                logs = docker_command(connection, "logs", identifier, checked=False)
                private_write(root / "private/supervisor.log", (logs.stdout + logs.stderr).decode(errors="replace"))
                docker_command(connection, "stop", "--time", "5", identifier, checked=False)
                if all(item["passed"] for item in evidence.checks):
                    docker_command(connection, "rm", identifier, checked=False)
                else:
                    resources["stoppedSupervisorRetainedForDiagnosis"] = True
        evidence.audit_public_files()
        write_json(root / "resources.json", resources)
        passed = all(item["passed"] for item in evidence.checks)
        write_json(root / "summary.json", {"status": "passed" if passed else "failed", "readyCommit": commit,
            "binarySha256": binary_sha256, "checksPassed": sum(x["passed"] for x in evidence.checks),
            "checksFailed": sum(not x["passed"] for x in evidence.checks), "resources": str(root / "resources.json"),
            "limitations": ["Unknown remote endpoint tested by actual configuration rejection; no remote daemon deployed"]})
    print(json.dumps({"status": "passed" if passed else "failed", "evidence": str(root), "image": image}))
    return 0 if passed else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--ready-commit", required=True)
    parser.add_argument("--binary", type=Path, help="Explicit newly approved binary; otherwise reuse the main run's unchanged pin")
    args = parser.parse_args()
    try:
        raise SystemExit(run(args))
    except Failure as error:
        print(json.dumps({"status": "refused", "reason": str(error)}))
        raise SystemExit(1)
