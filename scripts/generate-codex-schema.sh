#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
task_schema_dir=$(mktemp -d)
trap 'rm -rf "$task_schema_dir"' EXIT
"${CODEX_COMMAND:-codex}" app-server generate-json-schema --experimental --out "$task_schema_dir"
cp "$task_schema_dir/codex_app_server_protocol.v2.schemas.json" crates/runtime/src/native/codex/schema.json
cp "$task_schema_dir/ServerRequest.json" crates/runtime/src/native/codex/server-requests.schema.json
python3 scripts/generate-codex-types.py "$task_schema_dir"
