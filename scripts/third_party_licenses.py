#!/usr/bin/env python3
"""Writes THIRD-PARTY-LICENSES.txt: every crate that ends up in the release binary,
with its license expression and the license texts shipped in its source tree.

Redistributing a binary under MIT/BSD/Apache terms means including the notices of
what it contains - this keeps that file honest. Needs the crates already downloaded
(run `cargo fetch` or a build first). Usage:  python3 scripts/third_party_licenses.py
"""
import collections, hashlib, json, os, subprocess, sys

root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
meta = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", "x86_64-unknown-linux-gnu"],
    cwd=root))
pkgs = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

# Crates reachable through *normal* dependencies (build scripts / dev tools are not shipped).
seen, stack = set(), [meta["resolve"]["root"]]
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    stack += [d["pkg"] for d in nodes[i]["deps"] if any(k["kind"] is None for k in d["dep_kinds"])]
crates = sorted((pkgs[i] for i in seen if i != meta["resolve"]["root"]), key=lambda p: (p["name"], p["version"]))

texts = collections.OrderedDict()  # sha -> (text, [crates])
rows = []
for p in crates:
    d = os.path.dirname(p["manifest_path"])
    found = 0
    for f in sorted(os.listdir(d)):
        up = f.upper()
        if up.startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "NOTICE")) and os.path.isfile(os.path.join(d, f)):
            t = open(os.path.join(d, f), encoding="utf-8", errors="replace").read().strip()
            h = hashlib.sha256(" ".join(t.split()).encode()).hexdigest()
            texts.setdefault(h, (t, []))[1].append(f"{p['name']} {p['version']}")
            found += 1
    extra = ""
    if not found:  # no license file in the published crate: point at who holds the copyright
        who = ", ".join(a.split("<")[0].strip() for a in p.get("authors", [])[:3])
        extra = f"   [no license file in the crate; repository: {p.get('repository') or 'n/a'}" + (f"; authors: {who}" if who else "") + "]"
    rows.append((p["name"], p["version"], (p.get("license") or p.get("license_file") or "see source") + extra, found))

out = ["VeloEdit bundles the following third-party Rust crates.", "",
       "Two of them are under the Mozilla Public License 2.0 (nucleo-matcher, option-ext); their source is",
       "available unmodified at https://crates.io/crates/nucleo-matcher and https://crates.io/crates/option-ext.",
       "Crates offered under 'Apache-2.0 OR GPL-2.0-only' (self_cell) are used under Apache-2.0.", "",
       f"{len(rows)} crates", "=" * 72]
for name, ver, lic, n in rows:
    out.append(f"{name} {ver} - {lic}")
out += ["", "=" * 72, "LICENSE TEXTS", "=" * 72]
for t, users in texts.values():
    out += ["", "-" * 72, "Used by: " + ", ".join(users[:12]) + (f" and {len(users) - 12} more" if len(users) > 12 else ""), "-" * 72, t]
dest = os.path.join(root, "THIRD-PARTY-LICENSES.txt")
open(dest, "w", encoding="utf-8").write("\n".join(out) + "\n")
print(f"wrote {dest}: {len(rows)} crates, {len(texts)} distinct license texts, {os.path.getsize(dest) // 1024} KiB")
if any(n == 0 for *_, n in rows):
    print("note: some crates ship no license file:", ", ".join(f"{a} {b}" for a, b, _, n in rows if n == 0), file=sys.stderr)
