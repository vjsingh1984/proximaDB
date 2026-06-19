#!/usr/bin/env bash
# ProximaDB isolated-workspace helper.
# ----------------------------------------------------------------------------
# One task = one git worktree = one branch. Each unit of work gets its OWN
# working directory, branch, HEAD and index, branched from a known-good base
# (origin/develop). Concurrent sessions (humans or AI agents) therefore never
# share a checkout, so they cannot churn each other's HEAD, strand commits on
# the wrong branch, or clobber uncommitted WIP.
#
# Worktrees live in a SIBLING directory next to the repo (outside it — no
# .gitignore needed, no tooling scans them):
#   <repo-parent>/proximaDB.worktrees/<sanitized-branch>/
# Override with PROXIMADB_WORKTREE_ROOT.
#
# Usage:
#   scripts/worktree.sh new   <type/topic> [base]   # create + print path
#   scripts/worktree.sh list                         # list worktrees + state
#   scripts/worktree.sh rm    <type/topic> [--force] # remove (guards dirty)
#   scripts/worktree.sh clean                        # drop merged worktrees
#   scripts/worktree.sh guard                        # fail if in main checkout
#
# Typical flow (also the agent mandate — see CLAUDE.md):
#   eval "$(scripts/worktree.sh new feat/my-thing)"  # cd's you into it
#   ...edit, commit, push, open PR...
#   scripts/worktree.sh rm feat/my-thing             # after the PR merges
# ----------------------------------------------------------------------------
set -euo pipefail

BASE_DEFAULT="develop"
REMOTE="origin"

die() { printf 'worktree: %s\n' "$*" >&2; exit 1; }

# Resolve the MAIN repository checkout (the common dir's parent), regardless of
# which worktree we're invoked from.
repo_main() { git rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git/*$##'; }

worktree_root() {
  if [ -n "${PROXIMADB_WORKTREE_ROOT:-}" ]; then printf '%s' "$PROXIMADB_WORKTREE_ROOT"; return; fi
  printf '%s/proximaDB.worktrees' "$(dirname "$(repo_main)")"
}

sanitize() { printf '%s' "$1" | tr '/' '-' | tr -cs 'A-Za-z0-9._-' '-'; }

cmd_new() {
  local branch="${1:-}" base="${2:-$BASE_DEFAULT}"
  [ -n "$branch" ] || die "usage: new <type/topic> [base]   (e.g. feat/cloud-full)"
  case "$branch" in */*) : ;; *) die "branch must be <type>/<topic> (e.g. feat/$branch)";; esac
  local dir; dir="$(worktree_root)/$(sanitize "$branch")"
  [ -e "$dir" ] && die "worktree path already exists: $dir"
  git show-ref --verify --quiet "refs/heads/$branch" && die "branch already exists: $branch (use 'rm' first, or pick another)"

  git -C "$(repo_main)" fetch --quiet "$REMOTE" "$base" || die "cannot fetch $REMOTE/$base"
  mkdir -p "$(worktree_root)"
  git -C "$(repo_main)" worktree add -b "$branch" "$dir" "$REMOTE/$base" >&2
  # Emit a `cd` so callers can `eval "$(... new ...)"` to land in the worktree.
  printf 'cd %q\n' "$dir"
  printf 'worktree: %s  (branch %s off %s/%s)\n' "$dir" "$branch" "$REMOTE" "$base" >&2
}

cmd_list() {
  git -C "$(repo_main)" worktree list --porcelain | awk -v RS='' '
    { wt=""; br="(detached)";
      for (i=1;i<=split($0,L,"\n");i++) {
        if (L[i] ~ /^worktree /) { sub(/^worktree /,"",L[i]); wt=L[i] }
        if (L[i] ~ /^branch /)   { sub(/^branch refs\/heads\//,"",L[i]); br=L[i] }
      }
      printf "%-60s %s\n", wt, br
    }'
  echo "--- dirty? ---"
  while read -r wt; do
    [ -d "$wt" ] || continue
    local n; n="$(git -C "$wt" status --porcelain 2>/dev/null | wc -l | tr -d ' ')"
    [ "$n" != "0" ] && printf '  %s: %s uncommitted file(s)\n' "$wt" "$n"
  done < <(git -C "$(repo_main)" worktree list --porcelain | sed -n 's/^worktree //p')
  echo "(clean worktrees omitted)"
}

# Map a branch name -> its worktree path (empty if none).
wt_for_branch() {
  git -C "$(repo_main)" worktree list --porcelain | awk -v b="refs/heads/$1" '
    /^worktree /{p=$2} /^branch /{ if ($2==b) print p }'
}

cmd_rm() {
  local branch="${1:-}" force=""
  [ -n "$branch" ] || die "usage: rm <type/topic> [--force]"
  [ "${2:-}" = "--force" ] && force="--force"
  local dir; dir="$(wt_for_branch "$branch")"
  [ -n "$dir" ] || die "no worktree for branch: $branch"
  if [ -z "$force" ] && [ -n "$(git -C "$dir" status --porcelain 2>/dev/null)" ]; then
    die "worktree is dirty: $dir  (commit/stash, or pass --force to discard)"
  fi
  git -C "$(repo_main)" worktree remove ${force:+--force} "$dir"
  if git -C "$(repo_main)" branch --merged "$REMOTE/$BASE_DEFAULT" --format='%(refname:short)' | grep -qx "$branch"; then
    git -C "$(repo_main)" branch -d "$branch" && printf 'worktree: deleted merged branch %s\n' "$branch" >&2
  else
    printf 'worktree: removed %s (branch %s kept — not merged into %s)\n' "$dir" "$branch" "$BASE_DEFAULT" >&2
  fi
}

cmd_clean() {
  git -C "$(repo_main)" fetch --quiet "$REMOTE" "$BASE_DEFAULT" || true
  local removed=0
  while read -r dir; do
    [ "$dir" = "$(repo_main)" ] && continue
    local br; br="$(git -C "$dir" rev-parse --abbrev-ref HEAD 2>/dev/null || echo)"
    [ -n "$br" ] || continue
    if git -C "$(repo_main)" merge-base --is-ancestor "$br" "$REMOTE/$BASE_DEFAULT" 2>/dev/null; then
      [ -n "$(git -C "$dir" status --porcelain 2>/dev/null)" ] && { printf 'skip (dirty): %s\n' "$dir" >&2; continue; }
      git -C "$(repo_main)" worktree remove "$dir" && git -C "$(repo_main)" branch -D "$br" >/dev/null 2>&1 || true
      printf 'cleaned merged worktree: %s (%s)\n' "$dir" "$br" >&2
      removed=$((removed+1))
    fi
  done < <(git -C "$(repo_main)" worktree list --porcelain | sed -n 's/^worktree //p')
  git -C "$(repo_main)" worktree prune
  printf 'worktree: cleaned %d merged worktree(s)\n' "$removed" >&2
}

# Guardrail: refuse if the CWD is the MAIN checkout (agents call this before
# editing). A worktree's toplevel != the main checkout's toplevel.
cmd_guard() {
  local here; here="$(git rev-parse --show-toplevel 2>/dev/null || echo)"
  [ -n "$here" ] || die "not in a git repo"
  if [ "$here" = "$(repo_main)" ]; then
    die "you are in the MAIN checkout ($here). Do NOT edit here — run: scripts/worktree.sh new <type/topic>"
  fi
  printf 'worktree: OK — isolated workspace %s (branch %s)\n' "$here" "$(git rev-parse --abbrev-ref HEAD)"
}

case "${1:-}" in
  new)   shift; cmd_new "$@" ;;
  list)  shift; cmd_list "$@" ;;
  rm)    shift; cmd_rm "$@" ;;
  clean) shift; cmd_clean "$@" ;;
  guard) shift; cmd_guard "$@" ;;
  *) cat >&2 <<'USAGE'
worktree: one task = one worktree = one branch (isolated by construction)
  scripts/worktree.sh new   <type/topic> [base]   create + print `cd` (eval it)
  scripts/worktree.sh list                          list worktrees + dirty state
  scripts/worktree.sh rm    <type/topic> [--force]  remove (guards dirty)
  scripts/worktree.sh clean                         drop worktrees merged to develop
  scripts/worktree.sh guard                         fail if run in the main checkout
USAGE
     exit 2 ;;
esac
