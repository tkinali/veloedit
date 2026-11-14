#!/usr/bin/env bash
# Builds dist/veloedit-<version>-1.<arch>.rpm (x86_64 / aarch64) plus a .sha256 file.
#
#   cargo install cargo-generate-rpm --version 0.21.1 --locked     # once
#   scripts/package-rpm.sh
#
# The package metadata is in Cargo.toml ([package.metadata.generate-rpm]).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v cargo-generate-rpm >/dev/null || { echo "cargo-generate-rpm is missing: cargo install cargo-generate-rpm --version 0.21.1 --locked" >&2; exit 1; }
cargo build --release --locked
mkdir -p dist
rm -f dist/veloedit-*.rpm dist/veloedit-*.rpm.sha256
cargo generate-rpm --output dist/
rpm="$(ls dist/veloedit-*.rpm)"
(cd dist && sha256sum "$(basename "$rpm")" > "$(basename "$rpm").sha256")
echo "built $rpm ($(du -h "$rpm" | cut -f1))"
