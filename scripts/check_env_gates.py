#!/usr/bin/env python3
"""CI guard: every `PROXIMADB_*` env var referenced in Rust code must be
registered in docs/12-design/ENV_GATE_REGISTRY.adoc.

Freezes unregistered proliferation: a new gate lands only together with its
registry row (name, semantic, default, owner) — the naming rules live at the
top of the registry. Legacy rows may carry TODO semantics; NEW names may not
be added to code without a row at all.

Exit 0 = clean; exit 1 = unregistered vars (listed).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs/12-design/ENV_GATE_REGISTRY.adoc"
VAR_RE = re.compile(r"PROXIMADB_[A-Z0-9_]+")


def rust_vars():
    seen = set()
    for base in ("src", "crates", "apps"):
        for path in (ROOT / base).rglob("*.rs"):
            try:
                text = path.read_text(errors="ignore")
            except OSError:
                continue
            for m in VAR_RE.finditer(text):
                seen.add(m.group(0))
    return seen


def registered_vars():
    text = REGISTRY.read_text()
    return set(VAR_RE.findall(text))


def main():
    if not REGISTRY.exists():
        print(f"env-gate registry missing: {REGISTRY}", file=sys.stderr)
        return 1
    missing = sorted(rust_vars() - registered_vars())
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
