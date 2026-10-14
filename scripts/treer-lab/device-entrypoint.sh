#!/bin/sh
set -eu
umask 077
mkdir -p /workspace/lab-jobs /workspace/project /home/lab/.config /home/lab/.local/state
# Proxy is reachable only over the private Compose network; Controller uses loopback.
socat TCP-LISTEN:8787,bind=127.0.0.1,reuseaddr,fork TCP:proxy:8787 &
forward_pid=$!
trap 'kill "$forward_pid" 2>/dev/null || true' EXIT INT TERM
while kill -0 "$forward_pid" 2>/dev/null; do
    sleep 2
done
exit 1
