#!/usr/bin/env bash
# Builds the release binary and packs it into dist/veloedit-<version>-<arch>-linux.tar.gz
# (binary, installer, launcher entry, icons, licenses) plus a .sha256 file.
#
#   scripts/package.sh
#
# Build on the *oldest* distribution you want to support: the binary needs at least
# the glibc of the machine that built it.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
name="veloedit-$version-$arch-linux"

cargo build --release --locked
rm -rf "dist/$name" "dist/$name.tar.gz"
mkdir -p "dist/$name"
cp target/release/veloedit packaging/install.sh packaging/veloedit.desktop \
   assets/icon-128.png assets/icon-256.png LICENSE THIRD-PARTY-LICENSES.txt README.md "dist/$name/"
tar -C dist -czf "dist/$name.tar.gz" "$name"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")

echo "built dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
echo "needs glibc >= $(ldd --version | head -1 | grep -o '[0-9]\+\.[0-9]\+$') (this machine's)"
