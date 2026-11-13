#!/usr/bin/env bash
# Installs VeloEdit for the current user (no root needed): the program, its icon and
# a launcher entry, so it shows up in the application menu and in "Open with".
#
#   ./install.sh                  install into ~/.local
#   PREFIX=/usr/local sudo -E ./install.sh      system-wide
#   ./install.sh --uninstall      remove what was installed (same PREFIX)
#
# Works from the release tarball (files next to this script) and from a source
# checkout after `cargo build --release`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
bin="$prefix/bin/veloedit"
apps="$prefix/share/applications"
icons="$prefix/share/icons/hicolor"

# Where a file lives: next to this script (tarball) or in the source tree.
find_file() {
    for dir in "$here" "$here/../assets" "$here/../packaging" "$here/../target/release"; do
        [[ -f "$dir/$1" ]] && { echo "$dir/$1"; return 0; }
    done
    echo "install.sh: cannot find $1 (build first: cargo build --release)" >&2
    return 1
}

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$bin" "$apps/veloedit.desktop" "$icons/256x256/apps/veloedit.png" "$icons/128x128/apps/veloedit.png"
    echo "removed VeloEdit from $prefix (settings in ~/.config/veloedit and ~/.local/state/veloedit are kept)"
    exit 0
fi

src_bin="$(find_file veloedit)"
install -Dm755 "$src_bin" "$bin"
install -Dm644 "$(find_file icon-256.png)" "$icons/256x256/apps/veloedit.png"
install -Dm644 "$(find_file icon-128.png)" "$icons/128x128/apps/veloedit.png"
# The launcher must name the real binary: ~/.local/bin is not always on the desktop's PATH.
mkdir -p "$apps"
sed "s|^Exec=.*|Exec=\"$bin\" %F|" "$(find_file veloedit.desktop)" > "$apps/veloedit.desktop"
chmod 644 "$apps/veloedit.desktop"

command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$icons" 2>/dev/null || true

echo "installed: $bin"
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "note: $prefix/bin is not on your PATH - add it to run 'veloedit' from a terminal" ;;
esac
