#!/usr/bin/env python3
"""Native file authentication against private TLS protocol fixtures in the lab.

Requires a retained successful reincarnation run. Creates synthetic credentials;
never reads the operator's Codex/Claude/gh config or credentials. Public-range
addresses exist only inside the disposable nested daemon, to exercise the real
exact-host CONNECT resolver without changing host DNS/firewall or product code.
"""
import argparse, base64, ctypes, json, os, secrets, shutil, subprocess, time, urllib.request, http.cookiejar
from pathlib import Path
from reincarnation import Lab, ROOT, HERE, private, run, wait


def attach(manifest):
    m = json.loads(manifest.read_text())
    a = type("Args", (), {})()
    x = Lab.__new__(Lab)
    x.args = a
    x.manifest = m
    x.root = Path(m["runRoot"])
    x.lab = json.loads(
        (
            ROOT / ".scratch/git-environment-feasibility/agent-first/active-lab.json"
        ).read_text()
    )
    x.prefix = x.lab["prefix"]
    assert m["labPrefix"] == x.prefix and x.prefix.startswith("pocky-env-af-")
    x.docker = ["docker", "--host", x.lab["dockerHost"]]
    x.container = m["supervisorContainer"]
    x.base = m["apiBaseUrl"]
    x.secret_values = []
    x.evidence = []
    x.token = None
    v = json.loads((x.root / "private/owner.json").read_text())
    jar = http.cookiejar.CookieJar()
    o = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    o.open(
        urllib.request.Request(
            x.base + "/api/auth/login",
            data=json.dumps(
                {"username": "fixture-owner", "password": v["password"]}
            ).encode(),
            headers={"Content-Type": "application/json"},
        )
    ).read()
    x.token = next(iter(jar)).value
    x.secret_values.extend([x.token, v["password"], v["sessionSecret"]])
    return x


def jwt(payload):
    def encode(value):
        return (
            base64.urlsafe_b64encode(json.dumps(value, separators=(",", ":")).encode())
            .decode()
            .rstrip("=")
        )

    return (
        encode({"alg": "none", "typ": "JWT"})
        + "."
        + encode(payload)
        + "."
        + base64.urlsafe_b64encode(secrets.token_bytes(18)).decode().rstrip("=")
    )


def setup(x):
    x.native_root = x.root / "native-auth"
    x.native_root.mkdir()
    x.evidence = []
    x.check_original = x.check

    def check(name, value, details=None):
        assert value, "Native acceptance failed: " + name
        x.evidence.append({"name": name, "passed": True, "details": details or {}})
        (x.native_root / "evidence.json").write_text(
            json.dumps(x.evidence, indent=2) + "\n"
        )
        print("PASS", name, flush=True)

    x.check = check
    x.private = x.native_root / "private"
    x.private.mkdir(mode=0o700)
    x.sources = x.native_root / "sources"
    x.sources.mkdir(mode=0o700)
    for tool in ["codex", "claude", "gh"]:
        (x.sources / tool).mkdir(mode=0o700)
    now = int(time.time())
    id_token = jwt(
        {
            "email": "fixture@test.invalid",
            "exp": now + 3600,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "pro",
                "chatgpt_user_id": "synthetic-user",
                "chatgpt_account_id": "synthetic-account",
            },
        }
    )
    c = {
        k: secrets.token_urlsafe(40)
        for k in [
            "codexRefresh",
            "codexRotatedRefresh",
            "claudeAccess",
            "claudeRefresh",
            "claudeRotatedRefresh",
            "ghToken",
        ]
    }
    c["codexAccess"] = jwt({"exp": now + 3600})
    c["codexId"] = id_token
    private(x.private / "fixture.json", json.dumps(c))
    x.secret_values.extend(c.values())
    private(
        x.sources / "codex/auth.json",
        json.dumps(
            {
                "auth_mode": "chatgpt",
                "OPENAI_API_KEY": None,
                "tokens": {
                    "id_token": id_token,
                    "access_token": jwt({"exp": now - 7200}),
                    "refresh_token": c["codexRefresh"],
                    "account_id": "synthetic-account",
                },
                "last_refresh": "2025-01-01T00:00:00Z",
            }
        ),
    )
    private(x.sources / "codex/config.toml", "")
    private(
        x.sources / "claude/.credentials.json",
        json.dumps(
            {
                "claudeAiOauth": {
                    "accessToken": secrets.token_urlsafe(40),
                    "refreshToken": c["claudeRefresh"],
                    "expiresAt": (now - 3600) * 1000,
                    "scopes": [
                        "user:inference",
                        "user:profile",
                        "user:sessions:claude_code",
                    ],
                    "subscriptionType": "max",
                    "rateLimitTier": "default_claude_max_5x",
                }
            }
        ),
    )
    private(x.sources / "claude/settings.json", "{}\n")
    private(
        x.sources / "gh/hosts.yml",
        "github.com:\n    user: synthetic-owner\n    git_protocol: https\n    oauth_token: "
        + c["ghToken"]
        + "\n",
    )
    private(x.sources / "gh/config.yml", "version: 1\ngit_protocol: https\n")
    libc = ctypes.CDLL(None, use_errno=True)
    x.auth_watch = libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
    assert x.auth_watch >= 0
    for source in [
        x.sources / "codex/auth.json",
        x.sources / "claude/.credentials.json",
        x.sources / "gh/hosts.yml",
    ]:
        assert libc.inotify_add_watch(x.auth_watch, os.fsencode(source), 1 | 0x20) >= 0
    names = [
        "auth.openai.com",
        "chatgpt.com",
        "api.anthropic.com",
        "platform.claude.com",
        "github.com",
        "api.github.com",
    ]
    x.hosts = names
    run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            x.private / "tls.key",
            "-out",
            x.private / "ca.crt",
            "-days",
            "2",
            "-subj",
            "/CN=Pockymoe synthetic auth lab",
            "-addext",
            "subjectAltName=" + ",".join("DNS:" + n for n in names),
        ]
    )
    (x.private / "tls.key").chmod(0o600)
    (x.private / "ca.crt").chmod(0o644)
    run(
        [
            "openssl",
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            x.private / "server.key",
            "-out",
            x.private / "server.csr",
            "-subj",
            "/CN=auth.openai.com",
        ]
    )
    (x.private / "server.key").chmod(0o600)
    (x.private / "extensions.cnf").write_text(
        "basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName="
        + ",".join("DNS:" + n for n in names)
        + "\n"
    )
    run(
        [
            "openssl",
            "x509",
            "-req",
            "-in",
            x.private / "server.csr",
            "-CA",
            x.private / "ca.crt",
            "-CAkey",
            x.private / "tls.key",
            "-CAcreateserial",
            "-out",
            x.private / "server.crt",
            "-days",
            "2",
            "-extfile",
            x.private / "extensions.cnf",
        ]
    )
    x.network = x.prefix + "-native-auth"
    run(
        x.docker
        + [
            "network",
            "create",
            "--label",
            "pockymoe.testLab=" + x.prefix,
            "--subnet",
            "11.200.0.0/24",
            x.network,
        ]
    )
    run(x.docker + ["network", "connect", x.network, x.container])
    x.server = x.prefix + "-native-tls"
    run(
        x.docker
        + [
            "run",
            "-d",
            "--name",
            x.server,
            "--label",
            "pockymoe.testLab=" + x.prefix,
            "--cpus",
            ".25",
            "--memory",
            "192m",
            "--pids-limit",
            "64",
            "--network",
            x.network,
            "--ip",
            "11.200.0.3",
            "--user",
            "0:0",
            "--mount",
            f"type=bind,source={x.private},target=/fixture",
            "--mount",
            f"type=bind,source={HERE}/native_auth_fixture.py,target=/server.py,readonly",
            "--entrypoint",
            "python3",
            x.lab["nativeImage"],
            "/server.py",
            "--config",
            "/fixture/fixture.json",
            "--certificate",
            "/fixture/server.crt",
            "--key",
            "/fixture/server.key",
            "--evidence",
            "/fixture/events.json",
        ]
    )
    hosts = "11.200.0.3 " + " ".join(names) + "\n"
    r = subprocess.run(
        x.docker
        + [
            "exec",
            "-i",
            "--user",
            "0",
            x.container,
            "python3",
            "-c",
            "import sys;open('/etc/hosts','a').write(sys.stdin.read())",
        ],
        input=hosts.encode(),
        capture_output=True,
    )
    assert r.returncode == 0
    x.check(
        "synthetic-tls-origin-is-dedicated-nested-network",
        True,
        {"network": x.network, "server": x.server, "realService": False},
    )
    x.project = x.root / "workspaces/native-auth"
    x.project.mkdir()
    (x.project / ".fixture").mkdir()
    (x.project / "preserved.txt").write_text("keep this owner file untouched\n")
    shutil.copy2(HERE / "reincarnation_agent.py", x.project / ".fixture/handoff.py")
    shutil.copy2(
        HERE / "native_auth_probe.py", x.project / ".fixture/native_validation.py"
    )
    run(["git", "init", x.project])
    run(["git", "-C", x.project, "add", "."])
    run(
        [
            "git",
            "-C",
            x.project,
            "-c",
            "user.name=Synthetic fixture",
            "-c",
            "user.email=fixture@test.invalid",
            "commit",
            "-m",
            "native synthetic fixture baseline",
        ]
    )
    w = x.api(
        "/api/workspaces",
        {"absPath": str(x.project), "label": "Synthetic native file credentials"},
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
    x.id = t["id"]
    mounts = []
    for tool, purpose, pointer in [
        ("codex", "codex", "CODEX_HOME"),
        ("claude", "claude", "CLAUDE_CONFIG_DIR"),
        ("gh", "github", "GH_CONFIG_DIR"),
    ]:
        mounts.append(
            {
                "source": str(x.sources / tool),
                "target": "/credentials/" + tool,
                "readOnly": tool == "gh",
                "purpose": purpose,
                "environment": {pointer: "/credentials/" + tool},
            }
        )
    mounts.append(
        {
            "source": str(x.private / "ca.crt"),
            "target": "/credentials/fixture-ca.crt",
            "readOnly": True,
            "purpose": "generic",
            "environment": {
                k: "/credentials/fixture-ca.crt"
                for k in ["SSL_CERT_FILE", "NODE_EXTRA_CA_CERTS", "CURL_CA_BUNDLE"]
            },
        }
    )
    spec = {
        "connectionId": x.manifest["connectionId"],
        "name": "Synthetic native file authentication",
        "limits": {
            "cpu": 0.5,
            "memoryBytes": 1536 * 1024**2,
            "pids": 192,
            "nofile": 1024,
        },
        "authentication": {"codex": "nativeFile", "claude": "nativeFile"},
        "egressHosts": names,
        "credentialMounts": mounts,
        "handoff": "Synthetic validation only. Owner constraint: never modify preserved.txt. No actual subscription or paid model calls.",
    }
    (x.project / ".fixture/spec.json").write_text(json.dumps(spec))
    x.native_manifest = {
        "workspaceId": w["id"],
        "threadId": x.id,
        "project": str(x.project),
        "sources": str(x.sources),
        "network": x.network,
        "tlsContainer": x.server,
        "parentManifest": str(x.root / "manifest.json"),
    }
    (x.native_root / "manifest.json").write_text(
        json.dumps(x.native_manifest, indent=2) + "\n"
    )


def flow(x):
    x.api(
        "/api/threads/" + x.id + "/prompt",
        {
            "prompt": "fixture: propose native isolation. Owner constraint: never modify preserved.txt."
        },
    )
    r = wait(
        lambda: (
            json.loads((x.project / "proposal-receipt.json").read_text())
            if (x.project / "proposal-receipt.json").exists()
            else None
        )
    )
    x.native_manifest["requestId"] = r["id"]
    (x.native_root / "manifest.json").write_text(
        json.dumps(x.native_manifest, indent=2) + "\n"
    )
    x.check(
        "real-agent-native-proposal-contains-metadata-only",
        r["state"] == "pending"
        and r["spec"]["credentialMountKinds"]
        == ["directory", "directory", "directory", "file"],
    )
    try:
        accessed = bool(os.read(x.auth_watch, 65536))
    except BlockingIOError:
        accessed = False
    os.close(x.auth_watch)
    x.check("no-credential-file-open-or-read-before-owner-approval", not accessed)
    x.api("/api/environment-requests/" + r["id"] + "/approve", {"digest": r["digest"]})
    (x.project / "allow-host-finish").touch()
    ready = wait(
        lambda: (
            v
            if (v := x.api("/api/environment-requests/" + r["id"]))["state"]
            in ["ready", "failed"]
            else None
        ),
        110,
    )
    x.native_manifest["environmentId"] = ready.get("environmentId")
    x.native_manifest["requestState"] = ready["state"]
    (x.native_root / "manifest.json").write_text(
        json.dumps(x.native_manifest, indent=2) + "\n"
    )
    x.check(
        "native-authorized-direct-binds-worker-started",
        ready["state"] == "ready",
        {"state": ready["state"], "errorCategory": ready.get("lastError")},
    )
    value = wait(
        lambda: (
            json.loads((x.project / "native-auth-evidence.json").read_text())
            if (x.project / "native-auth-evidence.json").exists()
            else None
        ),
        180,
    )
    for name, passed in value.items():
        x.check(name, passed)
    environment = next(
        e
        for e in x.api("/api/environments")["environments"]
        if e["id"] == ready["environmentId"]
    )
    actual = json.loads(run(x.docker + ["inspect", environment["containerId"]]))[0]
    for mapping in r["spec"]["credentialMounts"]:
        bind = next(
            m for m in actual["Mounts"] if m["Destination"] == mapping["target"]
        )
        x.check(
            "direct-original-bind-" + mapping["purpose"],
            bind["Source"] == mapping["source"] and bind["RW"] != mapping["readOnly"],
        )
    # A second owner approval must not share a writable original credential
    # store even with a stopped native process. Its proposal can still be read.
    (x.project / "proposal-receipt.json").unlink()
    other = x.api(
        "/api/threads/start",
        {
            "workspaceId": x.native_manifest["workspaceId"],
            "provider": "acp",
            "agentId": "custom",
            "model": "fixture",
            "reasoningEffort": "max",
            "approvalMode": "yolo",
        },
    )
    x.api(
        "/api/threads/" + other["id"] + "/prompt",
        {
            "prompt": "fixture: propose a second synthetic credential grant; never modify preserved.txt."
        },
    )
    second = wait(
        lambda: (
            (
                v
                if (v := json.loads((x.project / "proposal-receipt.json").read_text()))[
                    "threadId"
                ]
                == other["id"]
                else None
            )
            if (x.project / "proposal-receipt.json").exists()
            else None
        )
    )
    x.api(
        "/api/environment-requests/" + second["id"] + "/approve",
        {"digest": second["digest"]},
        expected=400,
    )
    rejected = x.api(
        "/api/environment-requests/" + second["id"] + "/reject",
        {"digest": second["digest"]},
    )
    x.check(
        "same-source-rw-owner-grant-exclusive",
        rejected["state"] == "rejected" and not rejected.get("environmentId"),
    )
    events = json.loads((x.private / "events.json").read_text())
    (x.native_root / "protocol-events.json").write_text(
        json.dumps(events, indent=2) + "\n"
    )
    x.check(
        "codex-auth-refresh-used-native-refresh-token",
        any(
            e.get("tool") == "codex" and e.get("refresh") and e.get("authenticated")
            for e in events
        ),
    )
    x.check(
        "claude-auth-refresh-used-native-refresh-token",
        any(
            e.get("tool") == "claude" and e.get("refresh") and e.get("authenticated")
            for e in events
        ),
    )
    x.check(
        "claude-model-request-oauth-not-api-key",
        any(
            e.get("path") == "/v1/messages"
            and e.get("claudeAuthenticated")
            and e.get("apiKeyAbsent")
            for e in events
        ),
    )
    x.check(
        "gh-authenticated-local-tls-api",
        any(e.get("githubAuthenticated") for e in events),
    )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--run-manifest", type=Path, required=True)
    a = p.parse_args()
    x = attach(a.run_manifest)
    try:
        setup(x)
        flow(x)
    except Exception as e:
        message = (
            str(e)
            if isinstance(e, AssertionError)
            else "Private fixture failed; inspect redacted evidence, not native diagnostics"
        )
        (x.native_root / "failure.json").write_text(
            json.dumps({"category": type(e).__name__, "message": message}) + "\n"
        )
        print("FAIL", type(e).__name__, message, flush=True)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
