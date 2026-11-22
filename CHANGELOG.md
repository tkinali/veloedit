# Changelog

Every release has a section here; its text becomes the release notes on GitHub
(see [RELEASING.md](RELEASING.md)). Newest first.

## 0.2.0 - 2026-11-22

### Added
- **View → Syntax…** (also: click the language in the status bar, or *View: Select Syntax...* in the
  command palette) lists every language and applies the one you pick to the current tab. *Auto Detect*
  gives the decision back to the file name and content.
- A text in a new, unsaved tab that starts like a web page is colored right away: `<!DOCTYPE html>`,
  `<html>`, `<head>`, `<body>` (also after `<!-- saved from url=... -->` comments), other `<!DOCTYPE`s
  and `<svg` as XML, plus the `<?xml`, `<?php` and `#!` lines that were already recognized.
- **View → Convert Indentation to Spaces / Tabs** (uses the tab size of the settings) and
  **View → Line Endings: Unix (LF) / Windows (CRLF)**, also in the command palette. Each is one undo
  step (undoing a line-ending change also brings back the old style for new lines).

### Fixed
- A file with an unknown name whose text had a multi-byte character (such as `ş` or `日`) at the 64th
  byte could crash the editor when its language was guessed from its first bytes.

## 0.1.2 - 2026-11-21

### Fixed
- In a split tab, a pane's name bar with a long file name (or a long first line, for an unsaved
  text) no longer pushes its **×** and its unsaved-changes dot out of sight and out of reach: the
  name is cut instead, also in a very narrow pane.

## 0.1.1 - 2026-11-14

### Added
- **Windows 10/11 x64 build (preview).** `veloedit-<version>-x86_64-windows.zip`: unpack it anywhere and
  run `veloedit.exe`. It is much less tested than the Linux build and not code-signed (SmartScreen
  asks for confirmation). The single-window handshake uses a named pipe, the editor font is Consolas,
  and the git change marks need Git for Windows.
- **Linux packages:** `.deb` (amd64, arm64), `.rpm` (x86_64, aarch64) and `.AppImage` (x86_64, aarch64)
  next to the tarballs. The `.rpm` is unsigned: install it with `dnf install ./veloedit-*.rpm`.

### Changed
- Saving retries the final rename a few times on Windows (a virus scanner may hold the new file for a moment).

## 0.1.0 - 2026-11-13

First public release.

### Editor
- Rope-based text editor with multi-cursor editing (`Ctrl+Click`, `Ctrl+D`, `Ctrl+Alt+↑/↓`),
  regex find and replace, soft wrap, bracket matching, minimap, git gutter and an undo history
  that never loses a step.
- Code folding for every language (indentation based), gutter arrows, *Fold All* / *Fold Level N*.
- Split view (right / down) with a name bar per pane, drag-and-drop tab reordering.
- Syntax highlighting for about 50 languages: 25 tree-sitter grammars plus a light lexer.
- Hex editor for binary files (in-place byte patching) and an image viewer.
- Markdown preview next to the source, with real wrapping tables.
- *Trim trailing whitespace on save* (off by default, keeps Markdown's two-space line breaks).

### Project
- Multi-root folder sidebar with file-type badges and a right-click menu (new, rename, trash,
  copy path); preview tabs (single click) and permanent tabs (double click).
- `Ctrl+P` fuzzy file open, `Ctrl+Shift+P` command palette, `Ctrl+Shift+F` find in files with
  `*.php, !vendor` style filters.

### Robustness
- Hot exit: tabs, folders and unsaved drafts come back after a restart; unsaved tabs are named
  after their first line.
- One window: opening a file from the file manager hands it to the running editor.
- Safe on network mounts (SFTP/sshfs/NFS over a flaky VPN): a mount that stops answering never
  freezes the window, reloads from disk are undoable and never empty a buffer, and a frozen
  window does not block the next launch.
- Starts in well under 100 ms; the UI thread never waits for disk, git or highlighting.

### Packaging
- Linux x86_64 and aarch64 tarballs with an installer that needs no root (`install.sh`).
  Built on Ubuntu 22.04: needs glibc 2.35 or newer.
