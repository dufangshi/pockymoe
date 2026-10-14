#!/bin/sh
set -eu
# Treer identity remains inherited in memory. The data root belongs only to this lab.
unset POCKYMOE_RELAY_SERVER_URL POCKYMOE_RELAY_AGENT_TOKEN POCKYMOE_TOKEN POCKYMOE_THREAD_ID
export POCKYMOE_MODE=local POCKYMOE_HOST_MODE=hosted POCKYMOE_HOSTED_BY=treer
export POCKYMOE_E2E_FAKE_RUNTIME=1 HOST=127.0.0.1 PORT=18880
export POCKYMOE_DATABASE_PATH=/workspace/hub/device.sqlite
export POCKYMOE_WORKSPACE_ROOT=/workspace/project
export POCKYMOE_WEB_DIST_DIR=/opt/pockymoe-web
export POCKYMOE_SUPERVISOR_RESTART_STRATEGY=host-managed
mkdir -p /workspace/hub
exec /usr/local/bin/pockymoe supervisor
