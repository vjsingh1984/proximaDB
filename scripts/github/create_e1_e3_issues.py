#!/usr/bin/env python3
"""
Create or dry-run GitHub issues for the E1-E3 sprint board.

The script reads:
- docs/_internal/roadmap/implementation/active/E1_E3_GITHUB_ISSUE_PACK_2026_03_31.md
- docs/_internal/roadmap/implementation/active/E1_E3_SPRINT_BOARD_IMPORT_2026_03_31.csv

Default behavior is dry-run. Use --apply to actually open issues via `gh issue create`.
"""

from __future__ import annotations

import argparse
import csv
import re
import shlex
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parents[2]
ISSUE_PACK_PATH = (
    REPO_ROOT
    / "docs/_internal/roadmap/implementation/active/E1_E3_GITHUB_ISSUE_PACK_2026_03_31.md"
)
BOARD_CSV_PATH = (
    REPO_ROOT
    / "docs/_internal/roadmap/implementation/active/E1_E3_SPRINT_BOARD_IMPORT_2026_03_31.csv"
)

ISSUE_SECTION_SPLIT_RE = re.compile(r"^## (SB-\d+)\s*$", re.MULTILINE)


@dataclass
class IssueSpec:
    board_id: str
    epic: str
    title: str
    labels: list[str]
    lane: str
    estimate_days: str
    sprint: str
    depends_on: str
    parallel_with: str
    primary_files: str
    status: str
    body: str


def slugify(value: str) -> str:
    lowered = value.strip().lower()
    lowered = re.sub(r"[^a-z0-9]+", "-", lowered)
    return lowered.strip("-")


def parse_issue_pack(issue_pack_path: Path) -> dict[str, dict[str, str | list[str]]]:
    text = issue_pack_path.read_text(encoding="utf-8")
    issue_blocks: dict[str, dict[str, str | list[str]]] = {}
    matches = list(ISSUE_SECTION_SPLIT_RE.finditer(text))
    for index, match in enumerate(matches):
        board_id = match.group(1)
        start = match.end()
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        section = text[start:end]

        title_match = re.search(r"\*\*Title\*\*: (.+)", section)
        labels_match = re.search(r"\*\*Labels\*\*: (.+)", section)
        sprint_match = re.search(r"\*\*Sprint\*\*: (.+)", section)
        depends_match = re.search(r"\*\*Depends on\*\*: (.+)", section)
        parallel_match = re.search(r"\*\*Parallel with\*\*: (.+)", section)
        body_match = re.search(r"```md\n(.*?)\n```", section, re.DOTALL)

        if not all(
            [
                title_match,
                labels_match,
                sprint_match,
                depends_match,
                parallel_match,
                body_match,
            ]
        ):
            raise RuntimeError(f"Failed to parse issue-pack section for {board_id}")

        title_values = re.findall(r"`([^`]+)`", title_match.group(1))
        title = title_values[0] if title_values else title_match.group(1).strip()

        labels = re.findall(r"`([^`]+)`", labels_match.group(1))
        sprint_values = re.findall(r"`([^`]+)`", sprint_match.group(1))
        sprint = ", ".join(sprint_values) if sprint_values else sprint_match.group(1).strip()
        depends_values = re.findall(r"`([^`]+)`", depends_match.group(1))
        depends_on = ", ".join(depends_values) if depends_values else depends_match.group(1).strip()
        parallel_values = re.findall(r"`([^`]+)`", parallel_match.group(1))
        parallel_with = (
            ", ".join(parallel_values) if parallel_values else parallel_match.group(1).strip()
        )
        body = body_match.group(1)

        issue_blocks[board_id] = {
            "title": title,
            "labels": labels,
            "sprint": sprint,
            "depends_on": depends_on,
            "parallel_with": parallel_with,
            "body": body.strip(),
        }
    if not issue_blocks:
        raise RuntimeError(f"No issue blocks parsed from {issue_pack_path}")
    return issue_blocks


def parse_board_csv(csv_path: Path) -> dict[str, dict[str, str]]:
    rows: dict[str, dict[str, str]] = {}
    with csv_path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            board_id = row["board_id"]
            rows[board_id] = row
    if not rows:
        raise RuntimeError(f"No sprint-board rows parsed from {csv_path}")
    return rows


def build_issue_specs(issue_pack_path: Path, csv_path: Path) -> list[IssueSpec]:
    issue_blocks = parse_issue_pack(issue_pack_path)
    board_rows = parse_board_csv(csv_path)

    missing_in_csv = sorted(set(issue_blocks) - set(board_rows))
    missing_in_pack = sorted(set(board_rows) - set(issue_blocks))
    if missing_in_csv or missing_in_pack:
        raise RuntimeError(
            "Issue pack and board CSV are out of sync: "
            f"missing_in_csv={missing_in_csv}, missing_in_pack={missing_in_pack}"
        )

    specs: list[IssueSpec] = []
    for board_id in sorted(board_rows):
        row = board_rows[board_id]
        block = issue_blocks[board_id]
        specs.append(
            IssueSpec(
                board_id=board_id,
                epic=row["epic"],
                title=str(block["title"]),
                labels=list(block["labels"]),  # type: ignore[arg-type]
                lane=row["lane"],
                estimate_days=row["estimate_days"],
                sprint=row["target_sprint"],
                depends_on=row["depends_on"],
                parallel_with=row["parallel_with"],
                primary_files=row["primary_files"],
                status=row["status"],
                body=str(block["body"]),
            )
        )
    return specs


def render_issue_body(spec: IssueSpec) -> str:
    metadata_lines = [
        "## Sprint Board Metadata",
        f"- Board ID: `{spec.board_id}`",
        f"- Epic: `{spec.epic}`",
        f"- Lane: `{spec.lane}`",
        f"- Estimate: `{spec.estimate_days} days`",
        f"- Sprint: `{spec.sprint}`",
        f"- Depends On: `{spec.depends_on}`",
        f"- Parallel With: `{spec.parallel_with}`",
        f"- Status: `{spec.status}`",
        f"- Primary Files: `{spec.primary_files}`",
        f"- Source Matrix: `docs/_internal/roadmap/implementation/active/E1_E3_SPRINT_BOARD_MATRIX_2026_03_31.adoc`",
        f"- Source Issue Pack: `docs/_internal/roadmap/implementation/active/E1_E3_GITHUB_ISSUE_PACK_2026_03_31.md`",
        "",
    ]
    return "\n".join(metadata_lines) + "\n" + spec.body + "\n"


def derived_board_labels(spec: IssueSpec) -> list[str]:
    return [
        f"lane:{slugify(spec.lane)}",
        f"sprint:{slugify(spec.sprint)}",
        f"epic:{slugify(spec.epic)}",
    ]


def gather_labels(spec: IssueSpec, include_board_labels: bool) -> list[str]:
    labels = list(spec.labels)
    if include_board_labels:
        labels.extend(derived_board_labels(spec))
    deduped: list[str] = []
    for label in labels:
        if label not in deduped:
            deduped.append(label)
    return deduped


def color_for_label(label: str) -> str:
    if label.startswith("lane:"):
        return "1D76DB"
    if label.startswith("sprint:"):
        return "5319E7"
    if label.startswith("epic:"):
        return "0E8A16"
    if label == "epic":
        return "0E8A16"
    if label == "subtask":
        return "FBCA04"
    if label == "p0":
        return "B60205"
    if "quality" in label or "release" in label:
        return "5319E7"
    if "vector" in label or "index" in label:
        return "0052CC"
    if "api" in label or "query" in label or "planner" in label:
        return "1D76DB"
    return "6E7781"


def description_for_label(label: str) -> str:
    if label.startswith("lane:"):
        return f"Owner lane {label.split(':', 1)[1]}"
    if label.startswith("sprint:"):
        return f"Target sprint {label.split(':', 1)[1]}"
    if label.startswith("epic:"):
        return f"Epic grouping {label.split(':', 1)[1]}"
    if label == "epic":
        return "Execution epic"
    if label == "subtask":
        return "Sprint board subtask"
    if label == "p0":
        return "Highest priority"
    return f"Imported from sprint board metadata: {label}"


def run_gh(args: list[str], capture_output: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=REPO_ROOT,
        check=True,
        text=True,
        capture_output=capture_output,
    )


def ensure_label(label: str, repo: str | None, dry_run: bool) -> None:
    cmd = [
        "gh",
        "label",
        "create",
        label,
        "--color",
        color_for_label(label),
        "--description",
        description_for_label(label),
        "--force",
    ]
    if repo:
        cmd.extend(["--repo", repo])

    if dry_run:
        print(shlex.join(cmd))
    else:
        run_gh(cmd)


def issue_exists(title: str, repo: str | None, dry_run: bool) -> bool:
    if dry_run:
        return False

    cmd = [
        "gh",
        "issue",
        "list",
        "--state",
        "all",
        "--search",
        f'"{title}" in:title',
        "--json",
        "title",
        "--limit",
        "100",
    ]
    if repo:
        cmd.extend(["--repo", repo])
    result = run_gh(cmd, capture_output=True)
    output = result.stdout
    return f'"title":"{title}"' in output


def create_issue(spec: IssueSpec, repo: str | None, include_board_labels: bool, dry_run: bool) -> None:
    labels = gather_labels(spec, include_board_labels)
    body = render_issue_body(spec)

    with tempfile.NamedTemporaryFile("w", encoding="utf-8", suffix=".md", delete=False) as handle:
        handle.write(body)
        body_path = Path(handle.name)

    cmd = ["gh", "issue", "create", "--title", spec.title, "--body-file", str(body_path)]
    for label in labels:
        cmd.extend(["--label", label])
    if repo:
        cmd.extend(["--repo", repo])

    if dry_run:
        print(f"# {spec.board_id} {spec.title}")
        print(shlex.join(cmd))
        print()
    else:
        try:
            run_gh(cmd)
        finally:
            body_path.unlink(missing_ok=True)
        return

    body_path.unlink(missing_ok=True)


def parse_selection(raw_selection: str | None) -> set[str] | None:
    if not raw_selection:
        return None
    selected = {item.strip().upper() for item in raw_selection.split(",") if item.strip()}
    return selected or None


def filter_specs(
    specs: Iterable[IssueSpec],
    selected: set[str] | None,
    limit: int | None,
) -> list[IssueSpec]:
    filtered = [spec for spec in specs if selected is None or spec.board_id in selected]
    if limit is not None:
        filtered = filtered[:limit]
    return filtered


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Dry-run or create GitHub issues for the E1-E3 sprint board."
    )
    parser.add_argument("--repo", help="Optional GitHub repo in OWNER/REPO format.")
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Actually create issues. Default behavior is dry-run.",
    )
    parser.add_argument(
        "--select",
        help="Comma-separated list of board IDs to process, for example SB-01,SB-02.",
    )
    parser.add_argument(
        "--limit",
        type=int,
        help="Limit the number of issues processed after selection filtering.",
    )
    parser.add_argument(
        "--include-board-labels",
        action="store_true",
        help="Add derived labels for lane, sprint, and epic in addition to the issue-pack labels.",
    )
    parser.add_argument(
        "--ensure-labels",
        action="store_true",
        help="Create or update all labels before creating issues. Use with --apply or dry-run to print label commands.",
    )
    parser.add_argument(
        "--skip-existing",
        action="store_true",
        help="Skip issue creation when an issue with the exact same title already exists.",
    )
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    dry_run = not args.apply
    specs = build_issue_specs(ISSUE_PACK_PATH, BOARD_CSV_PATH)
    selected = parse_selection(args.select)
    specs = filter_specs(specs, selected, args.limit)

    if not specs:
        print("No issues selected.", file=sys.stderr)
        return 1

    if args.ensure_labels:
        all_labels: list[str] = []
        for spec in specs:
            for label in gather_labels(spec, args.include_board_labels):
                if label not in all_labels:
                    all_labels.append(label)
        for label in all_labels:
            ensure_label(label, args.repo, dry_run=dry_run)
        if dry_run:
            print()

    for spec in specs:
        if args.skip_existing and issue_exists(spec.title, args.repo, dry_run=dry_run):
            print(f"# Skipping existing issue: {spec.board_id} {spec.title}")
            continue
        create_issue(spec, args.repo, args.include_board_labels, dry_run=dry_run)

    if dry_run:
        print("# Dry-run complete. Re-run with --apply to create issues.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
