#!/usr/bin/env python3
"""CI guard: every `PROXIMADB_*` env var referenced in Rust code or the
container entrypoint/Dockerfiles must be registered in
docs/12-design/ENV_GATE_REGISTRY.adoc.

Freezes unregistered proliferation: a new gate lands only together with its
registry row (name, semantic, default, owner) — the naming rules live at the
top of the registry. Legacy rows may carry TODO semantics; NEW names may not
be added to code without a row at all.

The scan covers `*.rs` under src/crates/apps AND the deploy/docker entrypoint
shell + Dockerfiles — env vars consumed only by the container plumbing (never
read in Rust) would otherwise be invisible to the guard (the
`PROXIMADB_TIER_CONFIG_URL` family lived unregistered for exactly that
reason). Deliberately NOT scanned: scripts/ and helm/k8s YAML (deployment
plumbing without engine semantics — ~46 more names, registered as needed).

Exit 0 = clean; exit 1 = unregistered vars (listed).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs/12-design/ENV_GATE_REGISTRY.adoc"
VAR_RE = re.compile(r"PROXIMADB_[A-Z0-9_]+")

# Non-Rust surfaces the guard also scans: container plumbing that sets/reads
# PROXIMADB_* vars before the engine binary runs.
DEPLOY_GLOBS = ("deploy/docker/*.sh", "deploy/docker/Dockerfile*")


def scan_paths(paths):
    seen = set()
    for path in paths:
        try:
            text = path.read_text(errors="ignore")
        except OSError:
            continue
        for m in VAR_RE.finditer(text):
            name = m.group(0)
            # names ending in '_' are prose prefix mentions
            # ("PROXIMADB_QUEUE_* vars"), not variables
            if not name.endswith("_"):
                seen.add(name)
    return seen


def rust_vars():
    paths = []
    for base in ("src", "crates", "apps"):
        paths.extend((ROOT / base).rglob("*.rs"))
    return scan_paths(paths)


def deploy_vars():
    paths = []
    for pattern in DEPLOY_GLOBS:
        paths.extend(ROOT.glob(pattern))
    return scan_paths(paths)


def registered_vars():
    text = REGISTRY.read_text()
    return set(VAR_RE.findall(text))


def main():
    if not REGISTRY.exists():
        print(f"env-gate registry missing: {REGISTRY}", file=sys.stderr)
        return 1
    missing = sorted((rust_vars() | deploy_vars()) - registered_vars())
    if missing:
        print(
            "Unregistered PROXIMADB_* env gates (add a row to "
            "docs/12-design/ENV_GATE_REGISTRY.adoc — name, semantic, default, "
            "owner; naming rules at the top of that file):",
            file=sys.stderr,
        )
        for var in missing:
            print(f"  {var}", file=sys.stderr)
        return 1
    print(f"env-gate registry check passed: {len(registered_vars())} registered.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
