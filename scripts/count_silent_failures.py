#!/usr/bin/env python3
"""Ratchet the Python SDK's silent-failure sites.

The counterpart to `count_panic_patterns.sh`, which does this job for Rust
(`unwrap`/`expect`/`panic!`). This counts the Python shape of the same defect:

    except Exception:
        return []          # or {}, None, False, 0

A bare swallow that returns an empty or falsy value converts *any* failure into
a plausible answer. For a database read path that is the worst possible
outcome -- "no rows matched" and "the query failed" become the same thing, and
the caller acts on the difference.

Not every site is wrong. A capability probe may legitimately answer False, and a
fallback chain may legitimately try the next option. What is never acceptable is
that the caller cannot tell, so the count only ever goes DOWN: the baseline
records what remains and each is expected to carry a comment saying why.

Modes mirror the Rust script:
  report         -> metrics only, always exit 0
  no-regression  -> fail if the total exceeds the baseline
  baseline       -> rewrite the baseline from the current tree

The check is BIDIRECTIONAL. A total *below* the baseline also fails, because a
baseline that silently drifts down records nothing and stops being a ratchet --
the same lesson TD-CG2's one-directional language partition taught.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "clients" / "python" / "src" / "proximadb_sdk"
DEFAULT_BASELINE = REPO_ROOT / "docs" / "_internal" / "roadmap" / "SILENT_FAILURE_BASELINE.json"

#: `except (Exception|BaseException) [as e]:` followed only by comments and a
#: `return <falsy>`. Comments are skipped deliberately: a documented swallow is
#: still a swallow, and the point is to count them, not to reward prose.
PATTERN = re.compile(
    r"except\s+(?:Exception|BaseException)[^:]*:\s*\n"
    r"(?:[ \t]*#[^\n]*\n)*"
    r"[ \t]*return\s*(\[\]|\{\}|set\(\)|None|0|False)\s*(?:#[^\n]*)?\n"
)

#: Generated protobuf trees are not hand-written and are regenerated wholesale.
EXCLUDED_PARTS = ("_generated", "/v1/", "/v2/")


def find_sites() -> list[dict]:
    sites: list[dict] = []
    for path in sorted(SRC_ROOT.rglob("*.py")):
        rel = str(path.relative_to(REPO_ROOT))
        if any(part in rel for part in EXCLUDED_PARTS):
            continue
        text = path.read_text(errors="ignore")
        for match in PATTERN.finditer(text):
            sites.append(
                {
                    "file": str(path.relative_to(SRC_ROOT)),
                    "line": text[: match.start()].count("\n") + 1,
                    "returns": match.group(1),
                }
            )
    return sites


def summarise(sites: list[dict]) -> dict:
    by_file: dict[str, int] = {}
    for s in sites:
        by_file[s["file"]] = by_file.get(s["file"], 0) + 1
    return {"total": len(sites), "by_file": dict(sorted(by_file.items()))}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--mode", choices=("report", "no-regression", "baseline"), default="report")
    ap.add_argument("--baseline", type=Path, default=DEFAULT_BASELINE)
    args = ap.parse_args()

    sites = find_sites()
    current = summarise(sites)

    if args.mode == "baseline":
        args.baseline.parent.mkdir(parents=True, exist_ok=True)
        args.baseline.write_text(json.dumps(current, indent=2) + "\n")
        print(f"silent-failure baseline written: {current['total']} site(s) -> {args.baseline}")
        return 0

    print(f"Silent-failure sites in {SRC_ROOT.relative_to(REPO_ROOT)}: {current['total']}")
    for f, n in current["by_file"].items():
        print(f"  {n:3d}  {f}")

    if args.mode == "report":
        return 0

    if not args.baseline.exists():
        print(f"::error::no baseline at {args.baseline}; run --mode baseline", file=sys.stderr)
        return 1
    baseline = json.loads(args.baseline.read_text())
    expected = baseline["total"]

    if current["total"] > expected:
        added = [s for s in sites if s["file"] not in baseline["by_file"]
                 or current["by_file"][s["file"]] > baseline["by_file"].get(s["file"], 0)]
        print(
            f"::error::silent-failure sites rose {expected} -> {current['total']}. "
            "A bare `except: return <falsy>` makes a failure indistinguishable from "
            "an empty answer; record WHY in a comment and raise, log, or report it "
            "on the result instead.",
            file=sys.stderr,
        )
        for s in added[:10]:
            print(f"  candidate: {s['file']}:{s['line']} -> {s['returns']}", file=sys.stderr)
        return 1

    if current["total"] < expected:
        print(
            f"::error::silent-failure sites FELL {expected} -> {current['total']}. "
            "Good -- now lower the baseline in the same commit: "
            "`python3 scripts/count_silent_failures.py --mode baseline`. "
            "A baseline that drifts silently is not a ratchet.",
            file=sys.stderr,
        )
        return 1

    print(f"silent-failure ratchet holds at {expected}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
