#!/usr/bin/env python3
"""Inside-worker probe of real native CLIs using only approved synthetic binds.

Writes booleans only. OAuth values and native diagnostics are never returned.
"""
import json, os, select, subprocess, time, uuid, threading
from pathlib import Path

WORDS = [
    "certificate",
    "x509",
    "proxy",
    "EOF",
    "permission",
    "Permission",
    "authentication",
    "No such",
    "invalid",
    "Invalid",
    "connect",
    "command",
    "unavailable",
    "could",
    "not",
    "start",
    "credential",
    "helper",
    "config",
    "read-only",
    "parse",
    "read",
    "auth",
    "login",
    "file",
    "directory",
    "sandbox",
    "identity",
    "failed",
    "TLS",
    "unknown",
    "issuer",
    "error",
    "Error",
    "unsupported",
    "key",
    "user",
    "token",
    "public",
    "expires",
    "refresh",
    "scope",
    "OAuth",
    "system",
    "store",
]


def classes(data):
    return [k for k in WORDS if k.encode() in data]


def request(program, args, extra):
    env = {
        k: v
        for k, v in os.environ.items()
        if k
        in [
            "PATH",
            "HOME",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "POCKYMOE_ENVIRONMENT_WORKER",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "GH_CONFIG_DIR",
            "SSL_CERT_FILE",
            "NODE_EXTRA_CA_CERTS",
            "CURL_CA_BUNDLE",
        ]
    }
    env.update(extra)
    return {
        "id": str(uuid.uuid4()),
        "argv": [program, *args],
        "cwd": os.getcwd(),
        "env": list(env.items()),
    }


def native(command, input=None, extra=None, timeout=65):
    req = request(command[0], command[1:], extra or {})
    data = json.dumps(req) + "\n" + (input or "")
    p = subprocess.Popen(
        ["/opt/pockymoe/bin/pockymoe", "environment-worker", "run", req["id"]],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    out = []
    err = []

    def collect(stream, values):
        values.append(stream.read())

    readers = [
        threading.Thread(target=collect, args=(p.stdout, out)),
        threading.Thread(target=collect, args=(p.stderr, err)),
    ]
    for t in readers:
        t.start()
    p.stdin.write(data.encode())
    p.stdin.flush()
    try:
        p.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        p.terminate()
        p.wait(timeout=8)
    p.stdin.close()
    for t in readers:
        t.join(timeout=8)
    return subprocess.CompletedProcess(
        command, p.returncode, b"".join(out), b"".join(err)
    )


def main():
    results = {}
    for name, path in [
        ("ghReadOnlyBindEnforced", Path(os.environ["GH_CONFIG_DIR"]) / "config.yml"),
        ("genericReadOnlyFileEnforced", Path(os.environ["SSL_CERT_FILE"])),
    ]:
        try:
            with path.open("ab") as file:
                file.write(b"\n")
            results[name] = False
        except OSError:
            results[name] = True
    codex = Path(os.environ["CODEX_HOME"]) / "auth.json"
    claude = Path(os.environ["CLAUDE_CONFIG_DIR"]) / ".credentials.json"
    oldc = codex.read_bytes()
    olda = claude.read_bytes()
    inodec = codex.stat().st_ino
    inodea = claude.stat().st_ino
    # Native Codex app-server owns the refresh and file persistence. This does not
    # call a model or insert an OAuth token into an API-key profile.
    req = request(
        "/opt/pockymoe/bin/codex-subscription",
        ["app-server"],
        {"POCKYMOE_ENV_CODEX_NATIVE": "1"},
    )
    proc = subprocess.Popen(
        ["/opt/pockymoe/bin/pockymoe", "environment-worker", "run", req["id"]],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    try:
        proc.stdin.write(
            (
                json.dumps(req)
                + "\n"
                + json.dumps(
                    {
                        "id": 1,
                        "method": "initialize",
                        "params": {
                            "clientInfo": {
                                "name": "synthetic_oauth_probe",
                                "version": "1.0.0",
                            }
                        },
                    }
                )
                + "\n"
            ).encode()
        )
        proc.stdin.flush()

        def response(target):
            end = time.monotonic() + 65
            while time.monotonic() < end:
                if not select.select([proc.stdout], [], [], 0.25)[0]:
                    continue
                line = proc.stdout.readline()
                if not line:
                    break
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if value.get("id") == target:
                    return value
            return {}

        initialized = response(1)
        results["codexRealAppServerInitialized"] = "result" in initialized
        proc.stdin.write(
            (
                json.dumps({"method": "initialized"})
                + "\n"
                + json.dumps(
                    {
                        "id": 2,
                        "method": "account/read",
                        "params": {"refreshToken": True},
                    }
                )
                + "\n"
            ).encode()
        )
        proc.stdin.flush()
        account = response(2)
        results["codexNativeChatgptAccount"] = (
            account.get("result", {}).get("account", {}).get("type") == "chatgpt"
        )
        Path("native-diagnostic-metadata.json").write_text(
            json.dumps(
                {
                    "codexAccountResponseKeys": list(account),
                    "accountResultKeys": list(account.get("result", {})),
                    "accountKeys": list(
                        (account.get("result") or {}).get("account") or {}
                    ),
                    "errorCode": (account.get("error") or {}).get("code"),
                    "codexDiagnosticClasses": classes(
                        json.dumps(account.get("error", {})).encode()
                    ),
                    "envNames": sorted(os.environ),
                }
            )
            + "\n"
        )
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.terminate()
            proc.wait(timeout=8)
    results["codexRefreshPersistedInOriginalBind"] = codex.read_bytes() != oldc
    results["codexAtomicRename"] = codex.stat().st_ino != inodec
    # Native Claude CLI, not a fake executable or API-key route. Model output is a
    # deterministic local protocol fixture and incurs no subscription/API charge.
    r = native(
        [
            "/opt/pockymoe/harnesses/node_modules/@anthropic-ai/claude-agent-sdk-linux-x64/claude",
            "--print",
            "--output-format",
            "json",
            "--model",
            "claude-sonnet-4-6",
            "Reply with the synthetic fixture marker.",
        ],
        extra={
            "POCKYMOE_ENV_CLAUDE_NATIVE": "1",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        },
        timeout=80,
    )
    results["claudeRealCliOAuthStream"] = (
        r.returncode == 0 and b"SYNTHETIC_NATIVE_OAUTH_OK" in r.stdout
    )
    diag = json.loads(Path("native-diagnostic-metadata.json").read_text())
    diag["claudeExit"] = r.returncode
    diag["claudeDiagnosticClasses"] = [
        k
        for k in [
            "certificate",
            "unable to verify",
            "permission",
            "log in",
            "login",
            "proxy",
            "ECONN",
            "unrecognized",
            "Refresh",
            "refresh",
            "OAuth",
            "authentication",
            "No such",
            "lock",
            "invalid",
            "Invalid",
        ]
        if k.encode() in r.stderr + r.stdout
    ]
    Path("native-diagnostic-metadata.json").write_text(json.dumps(diag) + "\n")
    results["claudeRefreshPersistedInOriginalBind"] = claude.read_bytes() != olda
    results["claudeAtomicRename"] = claude.stat().st_ino != inodea
    r = native(["/usr/bin/gh", "api", "user"])
    results["ghRealFileAuth"] = r.returncode == 0 and b"synthetic-owner" in r.stdout
    diag["ghExit"] = r.returncode
    diag["ghDiagnosticClasses"] = classes(r.stderr)
    Path("native-diagnostic-metadata.json").write_text(json.dumps(diag) + "\n")
    r = native(
        ["/usr/bin/git", "credential", "fill"],
        input="protocol=https\nhost=github.com\n\n",
    )
    results["gitGhCredentialHelper"] = r.returncode == 0 and b"password=" in r.stdout
    Path("native-auth-evidence.json").write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
