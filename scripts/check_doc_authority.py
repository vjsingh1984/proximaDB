#!/usr/bin/env python3
"""Block customer-facing docs from contradicting the support contract.

The authoritative support contract is docs/SUPPORTED_SURFACE.adoc. Narrative
docs must not present retired engines as live, miscount the supported engines,
or list experimental engines/transports without the experimental caveat. This
turns "do the docs match reality?" into a gate instead of a manual audit — it
would have caught the 11-file drift fixed in #547 at PR time.

Rules are declarative (regex + allow-list), not asciidoc-table parsing (brittle):

  1. Retired graph engines PULSAR/QUASAR may appear ONLY in allow-listed
     retirement/migration docs. Anywhere else is a violation (use ORION).
  2. "6/six storage engines" is always wrong — there are 4 supported (SST, VIPER,
     HELIX, NOVA) plus 2 experimental (SWIFT, RAPTOR).
  3. SWIFT, RAPTOR, and Arrow Flight are experimental: any in-scope doc that
     mentions one of them must ALSO carry the word "experimental" (the
     off-by-default caveat). A doc listing them as supported without that caveat
     is a violation.

Exit 0 = clean, 1 = violation(s) found, 2 = usage error.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Customer-facing narrative docs in scope. Excludes the authority doc itself plus
# internal/historical/design trees (_internal, _archive, 12-design, 06-internals).
SCOPE_GLOBS = [
    "README.adoc",
    "SUPPORTED_SURFACE.md",
    "docs/README.md",
    "docs/INDEX.adoc",
    "docs/00-product/*.adoc",
    "docs/05-concepts/*.adoc",
    "docs/05-concepts/*.md",
    "docs/02-guides/*.md",
    "docs/01-quick-start/*.md",
    "docs/01-quick-start/*.adoc",
    "docs/03-api-reference/*.adoc",
]

# In-scope files permitted to mention retired engines (retirement/migration context).
RETIRED_ALLOW = {
    Path("README.adoc"),
    Path("SUPPORTED_SURFACE.md"),
    Path("docs/05-concepts/graph-engines.adoc"),
    Path("docs/01-quick-start/architecture-basics.md"),
}

RETIRED_RE = re.compile(r"\b(?:PULSAR|QUASAR)\b")
COUNT_RE = re.compile(r"\b(?:six|6)\s+storage[\s-]?engines?", re.IGNORECASE)
EXPERIMENTAL_TERMS = {
    "SWIFT": re.compile(r"\bSWIFT\b"),
    "RAPTOR": re.compile(r"\bRAPTOR\b"),
    "Arrow Flight": re.compile(r"\bArrow\s+Flight\b"),
}
EXPERIMENTAL_MARKER_RE = re.compile(r"\bexperimental\b", re.IGNORECASE)


def scope_files() -> list[Path]:
    seen: set[Path] = set()
    out: list[Path] = []
    for pat in SCOPE_GLOBS:
        for p in sorted((ROOT).glob(pat)):
            if p.is_file() and p not in seen:
                seen.add(p)
                out.append(p)
    return out


def rel(p: Path) -> str:
    try:
        return str(p.relative_to(ROOT))
    except ValueError:
        return str(p)


def check_file(p: Path) -> list[str]:
    try:
        text = p.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        return [f"{rel(p)}: could not read: {exc}"]
    lines = text.splitlines()
    rel_p = rel(p)
    violations: list[str] = []

    # Rule 1: retired engines only in allow-listed files.
    if Path(rel_p) not in RETIRED_ALLOW:
        for i, line in enumerate(lines, start=1):
            for m in RETIRED_RE.finditer(line):
                violations.append(
                    f"{rel_p}:{i}: retired engine '{m.group(0)}' — use ORION "
                    f"(allowed only in graph-engines/retirement docs)"
                )

    # Rule 2: engine-count claim is always wrong.
    for i, line in enumerate(lines, start=1):
        if COUNT_RE.search(line):
            violations.append(
                f"{rel_p}:{i}: claims '6/six storage engines' — there are 4 "
                f"supported (SST/VIPER/HELIX/NOVA); SWIFT/RAPTOR are experimental"
            )

    # Rule 3: experimental terms require the 'experimental' caveat in the same file.
    has_marker = bool(EXPERIMENTAL_MARKER_RE.search(text))
    for label, rx in EXPERIMENTAL_TERMS.items():
        first = next((i for i, line in enumerate(lines, start=1) if rx.search(line)), None)
        if first is not None and not has_marker:
            violations.append(
                f"{rel_p}:{first}: '{label}' is experimental but the file lacks the "
                f"'experimental' caveat — mark it off-by-default or drop the mention"
            )

    return violations


def main() -> int:
    violations: list[str] = []
    for p in scope_files():
        violations.extend(check_file(p))

    if violations:
        print(
            "ERROR: customer-facing docs contradict the support contract "
            "(docs/SUPPORTED_SURFACE.adoc is the authority):",
            file=sys.stderr,
        )
        for v in violations:
            print(f"  - {v}", file=sys.stderr)
        print(
            "\nFix the doc, not the gate. See docs/SUPPORTED_SURFACE.adoc for the "
            "supported/beta/experimental split.",
            file=sys.stderr,
        )
        return 1

    print(f"OK: doc-authority clean ({len(scope_files())} in-scope docs checked).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
