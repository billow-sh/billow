#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${BILLOW_INSTALL_URL:-}" ]]; then
    echo "BILLOW_INSTALL_URL must point to the billow archive" >&2
    exit 1
fi

temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/billow-install.XXXXXX")"
cleanup() {
    rm -rf "$temp_dir"
}
trap cleanup EXIT

archive="$temp_dir/billow.tar.gz"
curl -fsSL "$BILLOW_INSTALL_URL" -o "$archive"
tar -xzf "$archive" -C "$temp_dir"

cd "$temp_dir"

if [[ "$(id -u)" == "0" ]]; then
    ./billow-init
else
    sudo ./billow-init
fi
