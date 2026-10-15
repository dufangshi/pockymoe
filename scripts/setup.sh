#!/bin/sh
# Public bootstrap served by the relay from the same immutable runtime version.
# Keep this file ASCII: macOS /bin/sh (bash 3.2) reads a UTF-8 byte right after
# "$name" as part of the variable name and stops with "unbound variable".
set -eu
umask 077
relay_url=''
setup_token=''
setup_code=''
setup_port=8787
setup_reset_database=''
setup_web_origin=${POCKYMOE_WEB_ORIGIN:-}
setup_account_key=${POCKYMOE_ACCOUNT_KEY:-}
setup_enroll=${POCKYMOE_ENROLL_SECRET:-}
unset POCKYMOE_ENROLL_SECRET
while [ "$#" -gt 0 ]; do
  case "$1" in
    --account-key) setup_account_key=${2:?Missing account public key}; shift 2 ;;
    --enroll-stdin) [ -z "$setup_enroll" ] || { echo 'Choose stdin or POCKYMOE_ENROLL_SECRET.' >&2; exit 1; }; IFS= read -r setup_enroll || [ -n "$setup_enroll" ]; shift ;;
    --web-origin) setup_web_origin=${2:?Missing Web origin}; shift 2 ;;
    --relay) relay_url=${2:?Missing relay URL}; shift 2 ;;
    --token) setup_token=${2:?Missing device token}; shift 2 ;;
    --code) setup_code=${2:?Missing setup code}; shift 2 ;;
    --port) setup_port=${2:?Missing port}; shift 2 ;;
    --reset-database) setup_reset_database=1; shift ;;
    *) echo "Usage: setup.sh --relay URL (--token TOKEN | --code CODE) [--port PORT] [--web-origin URL] [--account-key KEY --enroll-stdin] [--reset-database] (or POCKYMOE_ENROLL_SECRET environment)" >&2; exit 1 ;;
  esac
done
[ -n "$relay_url" ] && { [ -n "$setup_token" ] || [ -n "$setup_code" ]; } || { echo 'Copy a setup command from the Devices page.' >&2; exit 1; }
[ -z "$setup_token" ] || [ -z "$setup_code" ] || { echo 'Choose either a device token or a setup code.' >&2; exit 1; }
{ [ -z "$setup_account_key" ] && [ -z "$setup_enroll" ]; } || { [ -n "$setup_account_key" ] && [ -n "$setup_enroll" ]; } || { echo 'Provide --account-key and enrollment through POCKYMOE_ENROLL_SECRET or --enroll-stdin.' >&2; exit 1; }
case "$relay_url" in https://*|http://localhost:*|http://127.0.0.1:*) ;; *) echo 'Relay must use HTTPS.' >&2; exit 1 ;; esac
case "$setup_port" in *[!0-9]*|'') echo 'Invalid port' >&2; exit 1 ;; esac
download() {
  if command -v curl >/dev/null 2>&1; then curl --fail --show-error --location --progress-bar --retry 2 --connect-timeout 20 --max-time 300 --speed-time 30 --speed-limit 1024 "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then wget --progress=bar:force --timeout=30 --tries=3 "$1" -O "$2"
  else echo 'curl or wget is required.' >&2; exit 1; fi
}
case "$(uname -s)" in Darwin) setup_os=darwin ;; Linux) setup_os=linux ;; *) echo 'This installer supports macOS and Linux.' >&2; exit 1 ;; esac
case "$(uname -m)" in arm64|aarch64) setup_arch=arm64 ;; x86_64|amd64) setup_arch=x64 ;; *) echo 'Unsupported CPU architecture.' >&2; exit 1 ;; esac
if [ "$setup_os" = darwin ] && [ "$setup_arch" != arm64 ]; then
  echo 'The macOS runtime currently requires Apple Silicon.' >&2; exit 1
fi
if [ "$setup_os" = linux ] && [ -f /etc/alpine-release ]; then
  echo 'The Linux runtime requires glibc (for example Ubuntu or Debian).' >&2; exit 1
fi
setup_root="$HOME/.local/share/remote-codex"
mkdir -p "$setup_root"
setup_tmp=$(mktemp -d "$setup_root/bootstrap.XXXXXX")
trap 'setup_exit=$?; rm -rf "$setup_tmp"; exit "$setup_exit"' EXIT
trap 'exit 1' HUP INT TERM
setup_repo=https://github.com/dufangshi/pockymoe
setup_asset="remote-codex-${setup_os}-${setup_arch}"
[ "$setup_os" != linux ] || setup_asset="$setup_asset-gnu"
echo 'Checking the latest Pockymoe GitHub release...'
if ! download "$setup_repo/releases/latest/download/runtime-version.txt" "$setup_tmp/version"; then
  echo 'Could not resolve the latest GitHub runtime release. Check your connection to github.com and retry.' >&2
  exit 1
fi
setup_version=$(tr -d '\r\n' < "$setup_tmp/version")
# Only an exact stable version can become part of a download URL or path.
if ! printf '%s\n' "$setup_version" | LC_ALL=C awk -F. 'NF != 3 {exit 1} {for (i=1;i<=3;i++) if ($i !~ /^[0-9]+$/) exit 1}'; then
  echo 'GitHub returned an invalid runtime version.' >&2
  exit 1
fi
echo "Downloading Pockymoe ${setup_version} (${setup_os}/${setup_arch})..."
setup_base="$setup_repo/releases/download/v$setup_version"
download "$setup_base/SHA256SUMS" "$setup_tmp/SHA256SUMS"
download "$setup_base/$setup_asset" "$setup_tmp/pockymoe"
setup_expected=$(awk -v name="$setup_asset" '$2 == name {count++; hash=$1} END {if(count == 1 && length(hash) == 64 && hash !~ /[^0-9a-fA-F]/) print tolower(hash); else exit 1}' "$setup_tmp/SHA256SUMS") || { echo 'Invalid or missing runtime checksum.' >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
  setup_actual=$(sha256sum "$setup_tmp/pockymoe" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  setup_actual=$(shasum -a 256 "$setup_tmp/pockymoe" | awk '{print $1}')
else
  echo 'sha256sum or shasum is required to verify the runtime.' >&2; exit 1
fi
[ "$setup_actual" = "$setup_expected" ] || { echo 'Runtime checksum verification failed. Nothing has been installed.' >&2; exit 1; }
chmod 700 "$setup_tmp/pockymoe"
if ! setup_actual_version=$("$setup_tmp/pockymoe" version); then
  echo 'Cannot execute the downloaded runtime. Linux requires glibc 2.28 or newer.' >&2; exit 1
fi
[ "$setup_actual_version" = "$setup_version" ] || { echo 'Runtime version verification failed.' >&2; exit 1; }
echo "Configuring Pockymoe ${setup_version}..."
# Do not exec: keep the cleanup trap alive until native setup finishes.
if [ -n "$setup_web_origin" ]; then
  export POCKYMOE_WEB_ORIGIN="$setup_web_origin"
fi
if [ -n "$setup_account_key" ]; then
  export POCKYMOE_ACCOUNT_KEY="$setup_account_key" POCKYMOE_ENROLL_SECRET="$setup_enroll"
fi
if [ -n "$setup_reset_database" ]; then
  set -- --reset-database
else
  set --
fi
if [ -n "$setup_token" ]; then
  "$setup_tmp/pockymoe" setup --relay "$relay_url" --token "$setup_token" --port "$setup_port" "$@"
else
  "$setup_tmp/pockymoe" setup --relay "$relay_url" --code "$setup_code" --port "$setup_port" "$@"
fi
