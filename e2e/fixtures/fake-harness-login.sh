#!/bin/sh
# Stands in for `codex` (CODEX_COMMAND) in harness-sign-in.spec.ts: Settings
# lists it as installed, and its sign-in, `codex login --device-auth`, runs in
# the real PTY of a sign-in window. It never signs anything in.
case "$1" in
  --version) echo "codex-cli 0.0.0-e2e"; exit 0 ;;
  login)
    printf 'Fake device login (%s)\r\n' "$2"
    printf 'Open https://example.invalid/device and enter ABCD-1234\r\n'
    printf 'Paste the confirmation: '
    read -r answer
    printf '\r\nSigned in as %s\r\n' "$answer"
    exit 0 ;;
esac
exit 1
