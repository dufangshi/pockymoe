#!/bin/bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
lab_state="$PWD/.local/treer-lab"
mkdir -p "$lab_state/native-postgres"
if [[ ! -x "$lab_state/native-postgres/usr/lib/postgresql/18/bin/postgres" ]]; then
    # Ubuntu 26.04 diagnostic fallback: unpack packages locally, never install them.
    (cd "$lab_state" && apt-get download postgresql-18 libpq5)
    for lab_deb in "$lab_state"/postgresql-18_*.deb "$lab_state"/libpq5_*.deb; do
        dpkg-deb -x "$lab_deb" "$lab_state/native-postgres"
    done
fi
if [[ ! -f "$lab_state/native-postgres-data/PG_VERSION" ]]; then
    LD_LIBRARY_PATH="$lab_state/native-postgres/usr/lib/x86_64-linux-gnu" \
        "$lab_state/native-postgres/usr/lib/postgresql/18/bin/initdb" \
        -D "$lab_state/native-postgres-data" \
        -L "$lab_state/native-postgres/usr/share/postgresql/18" --no-locale -A trust -U treer
fi
exec node scripts/treer-lab/native-smoke.mjs
