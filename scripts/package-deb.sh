#!/usr/bin/env bash
# Builds dist/veloedit_<version>-1_<arch>.deb (amd64 / arm64) plus a .sha256 file.
#
#   cargo install cargo-deb --version 3.8.0 --locked     # once
#   scripts/package-deb.sh
#
# Build on the *oldest* distribution you want to support (the binary needs at least the glibc of
# the machine that built it); the package metadata is in Cargo.toml ([package.metadata.deb]).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v cargo-deb >/dev/null || { echo "cargo-deb is missing: cargo install cargo-deb --version 3.8.0 --locked" >&2; exit 1; }
mkdir -p dist
rm -f dist/veloedit_*.deb dist/veloedit_*.deb.sha256
cargo deb --locked --output dist/
deb="$(ls dist/veloedit_*.deb)"
(cd dist && sha256sum "$(basename "$deb")" > "$(basename "$deb").sha256")
echo "built $deb ($(du -h "$deb" | cut -f1))"
