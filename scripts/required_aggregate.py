#!/usr/bin/env python3
"""Fail closed unless the final SHA's trusted aggregate succeeded on main.

Only the named aggregate job matters. A visible quarantine failure must not
turn a successful aggregate into a publication failure.
"""
import argparse
import hashlib
import json
import re
import subprocess

CONFIG = {
    "dufangshi/pockymoe": ("ci.yml", "PR checks complete"),
    "dufangshi/pockymoe-app": ("client-checks.yml", "Client checks complete"),
}


def api(path):
    try:
        response = subprocess.run(["gh", "api", path], check=True, text=True,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        return json.loads(response.stdout)
    except (subprocess.SubprocessError, ValueError):
        raise RuntimeError("Cannot verify required CI aggregate: GitHub query failed; publication is blocked") from None


def require_aggregate(repository, sha, override_reason=None, query=api):
    if repository not in CONFIG or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise RuntimeError("Required CI aggregate needs a known repository and full final SHA")
    workflow, check = CONFIG[repository]
    if override_reason is not None:
        if not override_reason.strip():
            raise RuntimeError("Emergency CI override requires a non-empty reason")
        # Never print arbitrary user text, which might contain private data.
        digest = hashlib.sha256(override_reason.encode()).hexdigest()
        print(f"EMERGENCY CI OVERRIDE (--allow-unchecked-ci): {check} bypassed for {sha}; reason SHA256={digest}", flush=True)
        return
    response = query(f"repos/{repository}/actions/workflows/{workflow}/runs?head_sha={sha}&branch=main&event=push&per_page=100")
    runs = [r for r in response.get("workflow_runs", []) if r.get("head_sha") == sha
            and r.get("event") == "push" and r.get("head_branch") == "main"
            and r.get("head_repository", {}).get("full_name") == repository
            and r.get("path", "").split("@")[0] == f".github/workflows/{workflow}"]
    message = f"Publication blocked: {check} must pass on the final main SHA {sha}. Wait for or repair that aggregate; quarantine is not a gate. Emergency use requires explicit --allow-unchecked-ci REASON."
    if not runs:
        raise RuntimeError(message)
    latest = max(runs, key=lambda r: (r["created_at"], r["id"]))
    attempt = latest.get("run_attempt", 1)
    jobs = query(f"repos/{repository}/actions/runs/{latest['id']}/attempts/{attempt}/jobs?per_page=100").get("jobs", [])
    selected = [j for j in jobs if j.get("name") == check]
    if len(selected) != 1 or selected[0].get("status") != "completed" or selected[0].get("conclusion") != "success":
        raise RuntimeError(message)
    print(f"Required aggregate passed: {check}; final SHA {sha}; run {latest['id']}, attempt {attempt}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, choices=CONFIG)
    parser.add_argument("--sha", required=True)
    parser.add_argument("--allow-unchecked-ci", metavar="REASON")
    args = parser.parse_args()
    try:
        require_aggregate(args.repository, args.sha, args.allow_unchecked_ci)
    except RuntimeError as error:
        parser.exit(1, str(error) + "\n")


if __name__ == "__main__":
    main()
