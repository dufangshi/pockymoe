#!/bin/sh
# Editable entry point; install.py, configure.py and profiles.json live beside it.
set -eu
if ! command -v python3 >/dev/null 2>&1; then
    echo 'Pockymoe setup: Python 3.11 or later is required.' >&2
    exit 1
fi
if ! python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))'; then
    echo 'Pockymoe setup: Python 3.11 or later is required.' >&2
    exit 1
fi
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
exec python3 "$script_dir/configure.py" --setup "$@"
