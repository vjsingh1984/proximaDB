#!/usr/bin/env bash
# Regression test for `scripts/worktree.sh clean` — the defensive (no-deletion-of-
# active-worktree) behavior. Runs entirely in a throwaway sandbox (a local
# file-path "origin" + cloned main repo under mktemp), so it touches nothing real.
#
# Reproduces incident 2026-07-30: `clean` deleted a freshly-created worktree
# mid-build because its branch tip == develop's tip (no commits yet), which
# `merge-base --is-ancestor` treats as "merged", and `target/` WIP is gitignored
# (invisible to `status --porcelain`).
#
#   bash scripts/test_worktree_clean.sh
# Exit 0 = all scenarios pass; 1 = at least one failed.
set -uo pipefail   # NOT -e: we assert explicitly and report every scenario.

REPO="$(git rev-parse --show-toplevel 2>/dev/null)"
[ -n "$REPO" ] || { echo "FAIL: run from within the repo"; exit 1; }
WT="$REPO/scripts/worktree.sh"

PASS=0; FAIL=0
ok()   { printf '  PASS: %s\n' "$1"; PASS=$((PASS+1)); }
bad()  { printf '  FAIL: %s\n' "$1"; FAIL=$((FAIL+1)); }

SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
BARE="$SANDBOX/origin.git"
MAIN="$SANDBOX/main"
WTS="$SANDBOX/wts"
export PROXIMADB_WORKTREE_ROOT="$WTS"
mkdir -p "$WTS"

# --- sandbox: bare origin + main repo on `develop` with target/ gitignored ---
git init --bare -q "$BARE"
git init -q "$MAIN"
git -C "$MAIN" checkout -q -b develop
git -C "$MAIN" config user.email test@test
git -C "$MAIN" config user.name test
# Isolate from the host's GLOBAL core.excludesFile (some machines ignore *.txt
# etc.), so only this repo's .gitignore (target/) applies in the sandbox.
git -C "$MAIN" config core.excludesFile /dev/null
printf 'target/\n' > "$MAIN/.gitignore"
git -C "$MAIN" add .gitignore
git -C "$MAIN" commit -q -m init
git -C "$MAIN" remote add origin "$BARE"
git -C "$MAIN" push -q -u origin develop
echo "sandbox: $MAIN (develop -> $BARE)"

new_wt() { ( cd "$MAIN" && bash "$WT" new "$1" >/dev/null 2>&1 ); }    # create a worktree (must run from sandbox repo so repo_main() resolves to it)
clean()  { ( cd "$MAIN" && bash "$WT" clean 2>&1 ); }

echo "== scenario 1: fresh, no-commit worktree with gitignored target/ is PROTECTED =="
new_wt chore/fresh
mkdir -p "$WTS/chore-fresh/target"; echo artifact > "$WTS/chore-fresh/target/foo"   # invisible to porcelain
out="$(clean)"
if [ -d "$WTS/chore-fresh" ]; then ok "fresh worktree survived clean"; else bad "fresh worktree was DELETED by clean"; fi
echo "$out" | grep -q 'protect (fresh/active' && ok "protect message emitted" || { bad "no protect message"; printf '    clean output: %s\n' "$out"; }

echo "== scenario 2: squash-merged worktree (distinct tip) IS reclaimed =="
new_wt feat/squashed
( cd "$WTS/feat-squashed" && printf 'content\n' > newfile.txt && git add newfile.txt && git commit -q -m "feat: add newfile" )
# GitHub-style squash: same change lands on develop as a NEW commit (new SHA)
( cd "$MAIN" && printf 'content\n' > newfile.txt && git add newfile.txt && git commit -q -m "feat: add newfile (#1)" && git push -q origin develop )
out="$(clean)"
[ -d "$WTS/feat-squashed" ] && bad "squash-merged worktree NOT reclaimed" || ok "squash-merged worktree reclaimed"
echo "$out" | grep -q 'feat-squashed' && ok "squash-merged reported in clean output" || true

echo "== scenario 3: merge-commit-merged worktree (proper ancestor) IS reclaimed =="
new_wt feat/mergecommit
( cd "$WTS/feat-mergecommit" && printf 'mc\n' > mcfile.txt && git add mcfile.txt && git commit -q -m "feat: mc" )
( cd "$MAIN" && git merge -q --no-ff feat/mergecommit -m "merge feat/mergecommit" && git push -q origin develop )
out="$(clean)"
[ -d "$WTS/feat-mergecommit" ] && bad "merge-commit worktree NOT reclaimed" || ok "merge-commit worktree reclaimed"

echo "== scenario 4: unmerged-ahead worktree (own commit, not landed) is PROTECTED =="
new_wt feat/wip
( cd "$WTS/feat-wip" && git commit -q --allow-empty -m "wip" )
out="$(clean)"
[ -d "$WTS/feat-wip" ] && ok "unmerged WIP worktree survived clean" || bad "unmerged WIP worktree was DELETED"

echo
echo "results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
