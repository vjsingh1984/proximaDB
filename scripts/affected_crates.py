#!/usr/bin/env python3
"""Print the workspace crates whose source changed vs a base ref.

Maps each changed file to the package whose manifest directory is the file's
longest path prefix, counting only source-ish files (src/, tests/, benches/,
build.rs, Cargo.toml) — so doc/CI/script edits map to no crate. Files under the
repo root's src/ map to the root `proximadb` crate.

This is DIRECT crates only — reverse-dependents are not expanded. For a change
to a low-level foundation crate, also run the full suite (`make test`); a
v2 can walk `cargo metadata`'s resolve graph for dependents.

Usage: affected_crates.py [base-ref]   (default: origin/develop)
Prints space-separated crate names to stdout (empty if no crate source changed).
"""
import json
import os
import subprocess
import sys


def git(args, cwd):
    return subprocess.run(["git", *args], capture_output=True, text=True, cwd=cwd).stdout


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/develop"
    root = git(["rev-parse", "--show-toplevel"], cwd=".").strip()
    if not root:
        sys.exit(0)
    # Committed changes vs base + any uncommitted working-tree changes.
    diff = git(["diff", "--name-only", f"{base}...HEAD"], root)
    diff += git(["diff", "--name-only", "HEAD"], root)
    changed = sorted({ln.strip() for ln in diff.splitlines() if ln.strip()})
    if not changed:
        return

    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            capture_output=True, text=True, cwd=root,
        ).stdout
    )
    # (manifest-dir-relative-with-trailing-slash, crate-name), longest dir first
    # so a nested crate wins over the root crate.
    pkgs = []
    for p in meta["packages"]:
        d = os.path.relpath(os.path.dirname(p["manifest_path"]), root)
        pkgs.append(("" if d == "." else d + "/", p["name"]))
    pkgs.sort(key=lambda x: -len(x[0]))

    def crate_for(f):
        for d, name in pkgs:
            if d == "":
                rest = f
            elif f.startswith(d):
                rest = f[len(d):]
            else:
                continue
            if rest in ("Cargo.toml", "build.rs") or rest.startswith(("src/", "tests/", "benches/")):
                return name
        return None

    seen = []
    for f in changed:
        c = crate_for(f)
        if c and c not in seen:
            seen.append(c)
    if seen:
        print(" ".join(seen))


if __name__ == "__main__":
    main()
