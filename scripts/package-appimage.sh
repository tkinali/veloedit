#!/usr/bin/env bash
# Builds dist/VeloEdit-<version>-<arch>.AppImage (x86_64 / aarch64) plus a .sha256 file.
#
#   scripts/package-appimage.sh
#
# One file that runs on most Linux desktops without installing anything. It needs FUSE on the
# machine that runs it (Ubuntu 22.04 and later: sudo apt install libfuse2); without FUSE run it as
#   ./VeloEdit-*.AppImage --appimage-extract-and-run
# Like the tarball it is built on Ubuntu 22.04 in CI, so it needs glibc 2.35 or newer.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
case "$arch" in
    x86_64)  tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
    aarch64) tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
    *) echo "no AppImage tool for $arch" >&2; exit 1 ;;
esac
# appimagetool is downloaded once, from its tagged release, and checked against the published checksum.
tool_version=1.9.1
tool="dist/tools/appimagetool-$tool_version-$arch.AppImage"
mkdir -p dist/tools
if [ ! -x "$tool" ]; then
    curl -fsSL -o "$tool" "https://github.com/AppImage/appimagetool/releases/download/$tool_version/appimagetool-$arch.AppImage"
    echo "$tool_sha  $tool" | sha256sum -c -
    chmod +x "$tool"
fi

cargo build --release --locked

app=dist/VeloEdit.AppDir
rm -rf "$app"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/256x256/apps"
cp target/release/veloedit "$app/usr/bin/"
cp packaging/veloedit.desktop "$app/usr/share/applications/"
cp packaging/veloedit.desktop "$app/veloedit.desktop"
cp assets/icon-256.png "$app/usr/share/icons/hicolor/256x256/apps/veloedit.png"
cp assets/icon-256.png "$app/veloedit.png"
ln -s veloedit.png "$app/.DirIcon"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/veloedit" "$@"
RUN
chmod +x "$app/AppRun"

out="dist/VeloEdit-$version-$arch.AppImage"
rm -f "$out" "$out.sha256"
# --appimage-extract-and-run: the tool itself is an AppImage and a CI machine has no FUSE.
ARCH="$arch" "$tool" --appimage-extract-and-run "$app" "$out"
chmod +x "$out"
(cd dist && sha256sum "$(basename "$out")" > "$(basename "$out").sha256")
echo "built $out ($(du -h "$out" | cut -f1))"
