#!/usr/bin/env python3
"""
Bulk-assign the E1-E3 sprint-board issues to a milestone and/or project.

The script can resolve issues in two ways:
- by exact title using the E1-E3 issue pack and sprint-board CSV
- by explicit issue numbers via --numbers

Default behavior is dry-run. Use --apply to execute GitHub mutations.
"""

from __future__ import annotations

import argparse
import json
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass
from typing import Iterable

from create_e1_e3_issues import REPO_ROOT, build_issue_specs, filter_specs, parse_selection
from create_e1_e3_issues import BOARD_CSV_PATH, ISSUE_PACK_PATH


REMOTE_RE = re.compile(
    r"^(?:git@github\.com:|https://github\.com/|ssh://git@github\.com/)([^/]+)/([^/]+?)(?:\.git)?$"
)


@dataclass
class ResolvedIssue:
    board_id: str
    title: str
    number: int
    url: str
    milestone: str | None


def run_cmd(args: list[str], capture_output: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=REPO_ROOT,
        check=True,
        text=True,
        capture_output=capture_output,
    )


def detect_repo_slug(explicit_repo: str | None) -> str:
    if explicit_repo:
        return explicit_repo

    result = run_cmd(["git", "config", "--get", "remote.origin.url"], capture_output=True)
    remote = result.stdout.strip()
    match = REMOTE_RE.match(remote)
    if not match:
        raise RuntimeError(
            "Could not determine GitHub repo from remote.origin.url. Pass --repo OWNER/REPO."
        )
    owner, repo = match.groups()
    return f"{owner}/{repo}"


def parse_numbers(raw_numbers: str | None) -> list[int] | None:
    if not raw_numbers:
        return None

    numbers: list[int] = []
    for chunk in raw_numbers.split(","):
        item = chunk.strip()
        if not item:
            continue
        if "-" in item:
            start_raw, end_raw = item.split("-", 1)
            start = int(start_raw)
            end = int(end_raw)
            if end < start:
                raise ValueError(f"Invalid range {item}: end is less than start")
            numbers.extend(range(start, end + 1))
        else:
            numbers.append(int(item))

    deduped: list[int] = []
    for number in numbers:
        if number not in deduped:
            deduped.append(number)
    return deduped or None


def list_repo_issues(repo: str) -> list[dict[str, object]]:
    cmd = [
        "gh",
        "issue",
        "list",
        "--repo",
        repo,
        "--state",
        "all",
        "--limit",
        "200",
        "--json",
        "number,title,url,milestone",
    ]
    result = run_cmd(cmd, capture_output=True)
    return json.loads(result.stdout)


def resolve_board_issues(
    repo: str,
    selected: set[str] | None,
    limit: int | None,
) -> list[ResolvedIssue]:
    specs = build_issue_specs(ISSUE_PACK_PATH, BOARD_CSV_PATH)
    specs = filter_specs(specs, selected, limit)
    title_map = {str(spec.title): spec.board_id for spec in specs}

    issues = list_repo_issues(repo)
    resolved: list[ResolvedIssue] = []
    for issue in issues:
        title = str(issue["title"])
        board_id = title_map.get(title)
        if not board_id:
            continue
        milestone_info = issue.get("milestone")
        milestone_title = None
        if isinstance(milestone_info, dict):
            milestone_title = milestone_info.get("title")
        resolved.append(
            ResolvedIssue(
                board_id=board_id,
                title=title,
                number=int(issue["number"]),
                url=str(issue["url"]),
                milestone=str(milestone_title) if milestone_title else None,
            )
        )

    resolved.sort(key=lambda issue: issue.board_id)
    missing = [spec.board_id for spec in specs if spec.board_id not in {r.board_id for r in resolved}]
    if missing:
        raise RuntimeError(
            "Could not resolve board issues in the target repo for: " + ", ".join(missing)
        )
    return resolved


def resolve_number_issues(numbers: list[int], repo: str) -> list[ResolvedIssue]:
    return [
        ResolvedIssue(
            board_id=f"manual-{number}",
            title=f"Issue {number}",
            number=number,
            url=f"https://github.com/{repo}/issues/{number}",
            milestone=None,
        )
        for number in numbers
    ]


def milestone_api_path(repo: str) -> str:
    return f"repos/{repo}/milestones"


def milestone_exists(repo: str, milestone: str) -> bool:
    result = run_cmd(
        [
            "gh",
            "api",
            "--method",
            "GET",
            milestone_api_path(repo),
            "--paginate",
            "-f",
            "state=all",
            "-f",
            "per_page=100",
        ],
        capture_output=True,
    )
    payload = json.loads(result.stdout)
    return any(item.get("title") == milestone for item in payload)


def ensure_milestone(
    repo: str,
    milestone: str,
    description: str | None,
    dry_run: bool,
) -> None:
    if dry_run:
        cmd = ["gh", "api", "-X", "POST", milestone_api_path(repo), "-f", f"title={milestone}"]
        if description:
            cmd.extend(["-f", f"description={description}"])
        print(shlex.join(cmd))
        return

    if milestone_exists(repo, milestone):
        return

    cmd = ["gh", "api", "-X", "POST", milestone_api_path(repo), "-f", f"title={milestone}"]
    if description:
        cmd.extend(["-f", f"description={description}"])
    run_cmd(cmd)


def assign_milestone(
    repo: str,
    issues: Iterable[ResolvedIssue],
    milestone: str,
    dry_run: bool,
) -> None:
    issue_numbers = [str(issue.number) for issue in issues if issue.milestone != milestone]
    if not issue_numbers:
        print(f"# All selected issues already use milestone {milestone!r}")
        return

    cmd = ["gh", "issue", "edit", *issue_numbers, "--repo", repo, "--milestone", milestone]
    if dry_run:
        print(shlex.join(cmd))
    else:
        run_cmd(cmd)


def assign_project_title(
    repo: str,
    issues: Iterable[ResolvedIssue],
    project_title: str,
    dry_run: bool,
) -> None:
    issue_numbers = [str(issue.number) for issue in issues]
    cmd = ["gh", "issue", "edit", *issue_numbers, "--repo", repo, "--add-project", project_title]
    if dry_run:
        print(shlex.join(cmd))
    else:
        run_cmd(cmd)


def assign_project_number(
    project_owner: str,
    project_number: int,
    issues: Iterable[ResolvedIssue],
    dry_run: bool,
) -> None:
    for issue in issues:
        cmd = [
            "gh",
            "project",
            "item-add",
            str(project_number),
            "--owner",
            project_owner,
            "--url",
            issue.url,
        ]
        if dry_run:
            print(shlex.join(cmd))
        else:
            run_cmd(cmd)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Dry-run or apply milestone/project assignment for E1-E3 issues."
    )
    parser.add_argument("--repo", help="Optional GitHub repo in OWNER/REPO format.")
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Actually mutate GitHub state. Default behavior is dry-run.",
    )
    parser.add_argument(
        "--select",
        help="Comma-separated board IDs, for example SB-01,SB-02. Ignored with --numbers.",
    )
    parser.add_argument(
        "--limit",
        type=int,
        help="Limit selected board issues after filtering. Ignored with --numbers.",
    )
    parser.add_argument(
        "--numbers",
        help="Explicit issue numbers or ranges, for example 31-50 or 31,32,35.",
    )
    parser.add_argument("--milestone", help="Milestone name to apply to all selected issues.")
    parser.add_argument(
        "--ensure-milestone",
        action="store_true",
        help="Create the milestone first if it does not already exist.",
    )
    parser.add_argument(
        "--milestone-description",
        help="Optional milestone description used with --ensure-milestone.",
    )
    parser.add_argument(
        "--project-title",
        help="Add selected issues to a project by title using `gh issue edit --add-project`.",
    )
    parser.add_argument(
        "--project-owner",
        help="Project owner login for `gh project item-add`. Requires --project-number.",
    )
    parser.add_argument(
        "--project-number",
        type=int,
        help="Project number for `gh project item-add`. Requires --project-owner.",
    )
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()

    if not args.milestone and not args.project_title and not args.project_number:
        parser.error("Provide at least one of --milestone, --project-title, or --project-number.")

    if args.project_title and (args.project_owner or args.project_number):
        parser.error("Use either --project-title or --project-owner/--project-number, not both.")

    if (args.project_owner is None) != (args.project_number is None):
        parser.error("--project-owner and --project-number must be provided together.")

    repo = detect_repo_slug(args.repo)
    dry_run = not args.apply

    explicit_numbers = parse_numbers(args.numbers)
    if explicit_numbers:
        issues = resolve_number_issues(explicit_numbers, repo)
    else:
        selected = parse_selection(args.select)
        issues = resolve_board_issues(repo, selected, args.limit)

    print(f"# Repo: {repo}")
    print(
        "# Selected issues: "
        + ", ".join(f"{issue.board_id}=>#{issue.number}" for issue in issues)
    )

    if args.milestone and args.ensure_milestone:
        ensure_milestone(repo, args.milestone, args.milestone_description, dry_run)

    if args.milestone:
        assign_milestone(repo, issues, args.milestone, dry_run)

    if args.project_title:
        assign_project_title(repo, issues, args.project_title, dry_run)

    if args.project_owner and args.project_number:
        assign_project_number(args.project_owner, args.project_number, issues, dry_run)

    if dry_run:
        print("# Dry-run complete. Re-run with --apply to execute.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
