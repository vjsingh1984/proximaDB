#!/usr/bin/env python3
"""Block AI-agent authorship attribution in commit messages and PR text.

Policy (the human drives the code, not the agents): commit titles/bodies and PR
title/body must not carry AI-agent *authorship attribution* — co-author trailers
naming an agent, "Generated with ..." taglines, the robot emoji, agent-email
trailers, or agent signature strings (e.g. "Claude Opus 4.8", "Claude Code",
"Gemini Pro").

This targets *attribution*, not mere mention: legitimate references such as the
`CLAUDE.md`/`GEMINI.md`/`AGENTS.md` rule files or integrating the Anthropic/OpenAI
APIs are allowed. Adjust FORBIDDEN_PATTERNS if you want a stricter bare-word rule.

Modes:
  --message-file <path>   check a single commit message file (commit-msg hook)
  --range <base>..<head>  check every commit message in the range (CI)
  --stdin                 check text read from stdin (e.g. PR title+body)

Exit 0 = clean, 1 = violation(s) found, 2 = usage error.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

# Case-insensitive attribution patterns. Each entry: (compiled regex, human label).
_AGENTS = r"claude|codex|gemini|copilot|chatgpt|gpt-\d|anthropic|openai|bard|llm"
FORBIDDEN_PATTERNS = [
    (re.compile(rf"^\s*co-authored-by:.*(?:{_AGENTS})", re.I | re.M),
     "co-author trailer naming an AI agent"),
    (re.compile(r"generated\s+with\s+\[?\s*(?:claude|codex|gemini|copilot|chatgpt)", re.I),
     '"Generated with <agent>" tagline'),
    (re.compile(r"🤖"),
     "robot emoji (agent-generated marker)"),
    (re.compile(r"(?:authored|written|created|generated|produced)\s+by\s+(?:claude|codex|gemini|copilot|chatgpt|anthropic|openai)", re.I),
     '"<verb> by <agent>" attribution'),
    (re.compile(r"\b(?:claude|gemini)\s+(?:opus|sonnet|haiku|code|pro|flash|\d)", re.I),
     "agent model/product signature (e.g. 'Claude Opus', 'Claude Code')"),
    (re.compile(r"noreply@anthropic\.com|noreply@openai\.com|@users\.noreply\.github\.com.*(?:claude|copilot)", re.I),
     "agent no-reply email trailer"),
    (re.compile(r"\bco-authored-by:\s*(?:claude|copilot|codex)\b", re.I),
     "agent co-author trailer"),
]


def scan(text: str, source: str) -> list[str]:
    """Return a list of violation descriptions for the given text."""
    violations: list[str] = []
    for rx, label in FORBIDDEN_PATTERNS:
        for m in rx.finditer(text):
            line = m.group(0).strip().replace("\n", " ")
            violations.append(f"{source}: {label} -> '{line[:120]}'")
    return violations


def commit_messages_in_range(rng: str) -> list[tuple[str, str]]:
    """Return [(sha, message)] for each commit in the range."""
    out = subprocess.run(
        ["git", "rev-list", "--no-merges", rng],
        capture_output=True, text=True, check=True,
    ).stdout.split()
    msgs = []
    for sha in out:
        msg = subprocess.run(
            ["git", "log", "-1", "--format=%B", sha],
            capture_output=True, text=True, check=True,
        ).stdout
        msgs.append((sha, msg))
    return msgs


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    g = ap.add_mutually_exclusive_group(required=True)
    g.add_argument("--message-file")
    g.add_argument("--range")
    g.add_argument("--stdin", action="store_true")
    args = ap.parse_args()

    violations: list[str] = []
    if args.message_file:
        with open(args.message_file, encoding="utf-8", errors="replace") as fh:
            violations += scan(fh.read(), "commit message")
    elif args.stdin:
        violations += scan(sys.stdin.read(), "PR title/body")
    else:
        try:
            for sha, msg in commit_messages_in_range(args.range):
                violations += scan(msg, f"commit {sha[:9]}")
        except subprocess.CalledProcessError as exc:
            print(f"error: could not read commit range '{args.range}': {exc}", file=sys.stderr)
            return 2

    if violations:
        print("ERROR: AI-agent authorship attribution is not allowed in commit/PR text.", file=sys.stderr)
        print("The human drives the code; remove the agent attribution below:\n", file=sys.stderr)
        for v in violations:
            print(f"  - {v}", file=sys.stderr)
        print("\n(Mentions of CLAUDE.md/GEMINI.md or the Anthropic/OpenAI APIs are fine; "
              "this blocks authorship attribution only. See scripts/check_no_agent_attribution.py.)",
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
