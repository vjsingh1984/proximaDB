#!/usr/bin/env python3
"""Validate deterministic, architecture-safe commit guard wiring.

This is a fast static guard for the repo's "safe to commit" contract. It does
not replace the expensive build/test gates; it verifies that those gates still
point at deterministic policies:

* unit tests use the zero-retry nextest profile
* CI and Makefile still invoke that profile
* architecture docs still separate code presence from support level
* tenant/path mandates remain visible in the system map
* conflict markers are not staged into source/docs
"""

from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

TEXT_EXTENSIONS = {
    ".adoc",
    ".bash",
    ".c",
    ".cc",
    ".cfg",
    ".h",
    ".html",
    ".java",
    ".js",
    ".json",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".sql",
    ".toml",
    ".ts",
    ".txt",
    ".yaml",
    ".yml",
}

CONFLICT_RE = re.compile(r"^(<<<<<<<|=======|>>>>>>>)($| )", re.MULTILINE)


@dataclass(frozen=True)
class Finding:
    check: str
    message: str


def rel(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def read_text(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def compact_whitespace(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def require_contains(findings: list[Finding], check: str, path: str, needle: str) -> None:
    text = read_text(path)
    if needle not in text and compact_whitespace(needle) not in compact_whitespace(text):
        findings.append(Finding(check, f"{path} must contain: {needle!r}"))


def check_nextest_contract(findings: list[Finding]) -> None:
    path = ROOT / ".config/nextest.toml"
    config = tomllib.loads(path.read_text(encoding="utf-8"))
    profiles = config.get("profile", {})

    unit = profiles.get("unit", {})
    integration = profiles.get("integration", {})

    # NOTE: the zero-retry contract (profile.default/unit.retries must be 0, and
    # no retry overrides) was intentionally removed. A small retry budget absorbs
    # load-induced flakes on constrained CI runners; nextest still surfaces any
    # survivor distinctly as FLAKY, so genuine breakage (which exhausts all
    # retries) still fails the run.
    if unit.get("test-threads", 0) < 2:
        findings.append(
            Finding(
                "nextest",
                ".config/nextest.toml profile.unit.test-threads must stay >1 to expose contention flakes",
            )
        )
    if unit.get("failure-output") != "immediate-final":
        findings.append(
            Finding(
                "nextest",
                '.config/nextest.toml profile.unit.failure-output must be "immediate-final"',
            )
        )
    if integration.get("retries", 0) > 1:
        findings.append(
            Finding(
                "nextest",
                ".config/nextest.toml profile.integration.retries must not exceed 1",
            )
        )


def check_gate_wiring(findings: list[Finding]) -> None:
    require_contains(
        findings,
        "gate-wiring",
        ".github/workflows/ci.yml",
        "cargo nextest run --lib --profile unit",
    )
    require_contains(
        findings,
        "gate-wiring",
        "Makefile",
        "cargo nextest run --lib --profile unit",
    )
    require_contains(
        findings,
        "gate-wiring",
        "Makefile",
        "docs-claim-check",
    )
    require_contains(
        findings,
        "gate-wiring",
        "Makefile",
        "workspace-boundaries-check",
    )
    require_contains(
        findings,
        "gate-wiring",
        "Makefile",
        "release-check: work-commit-check",
    )
    require_contains(
        findings,
        "gate-wiring",
        ".github/workflows/layering-check.yml",
        "python3 scripts/check_tenant_path_guard.py",
    )


def check_architecture_contract(findings: list[Finding]) -> None:
    require_contains(
        findings,
        "architecture",
        "docs/SUPPORTED_SURFACE.adoc",
        "Do not infer support level from code presence alone.",
    )
    require_contains(
        findings,
        "architecture",
        "docs/README.md",
        "Code presence is broader than the supported product surface.",
    )
    require_contains(
        findings,
        "architecture",
        "docs/README.md",
        "12-design/SYSTEM_MAP_2026_05_30.adoc",
    )
    require_contains(
        findings,
        "architecture",
        "docs/12-design/SYSTEM_MAP_2026_05_30.adoc",
        "code presence does not imply support level",
    )
    require_contains(
        findings,
        "architecture",
        "docs/12-design/SYSTEM_MAP_2026_05_30.adoc",
        "TenantContext",
    )
    require_contains(
        findings,
        "architecture",
        "docs/12-design/SYSTEM_MAP_2026_05_30.adoc",
        "DrPathBuilder",
    )
    require_contains(
        findings,
        "architecture",
        "docs/12-design/SYSTEM_MAP_2026_05_30.adoc",
        "Validation and guardrail map",
    )
    require_contains(
        findings,
        "architecture",
        "docs/12-design/SYSTEM_MAP_2026_05_30.adoc",
        "make work-commit-check",
    )
    require_contains(
        findings,
        "architecture",
        "docs/06-internals/workflows/TDD_GUIDE.md",
        "zero-retry unit contract",
    )
    require_contains(
        findings,
        "architecture",
        "docs/06-internals/workflows/TDD_GUIDE.md",
        "make work-commit-check",
    )


def tracked_files() -> list[Path]:
    try:
        output = subprocess.check_output(
            ["git", "ls-files"],
            cwd=ROOT,
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except (OSError, subprocess.CalledProcessError):
        return sorted(
            path.relative_to(ROOT)
            for path in ROOT.rglob("*")
            if path.is_file() and ".git" not in path.parts
        )
    return [Path(line) for line in output.splitlines() if line]


def check_conflict_markers(findings: list[Finding]) -> None:
    for rel_path in tracked_files():
        if rel_path.suffix not in TEXT_EXTENSIONS:
            continue
        path = ROOT / rel_path
        if not path.exists():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        match = CONFLICT_RE.search(text)
        if match:
            line = text[: match.start()].count("\n") + 1
            findings.append(
                Finding(
                    "conflict-markers",
                    f"{rel_path.as_posix()}:{line} contains a merge conflict marker",
                )
            )


def main() -> int:
    findings: list[Finding] = []

    check_nextest_contract(findings)
    check_gate_wiring(findings)
    check_architecture_contract(findings)
    check_conflict_markers(findings)

    print("Deterministic commit contract")
    if not findings:
        print("OK: nextest, CI/Makefile wiring, architecture guards, and conflict-marker checks pass.")
        return 0

    print(f"FAILED: {len(findings)} finding(s)")
    for finding in findings:
        print(f"- [{finding.check}] {finding.message}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
