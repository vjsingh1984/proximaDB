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
#   scripts/worktree.sh clean [--dry-run]            # reclaim merged/squashed worktrees
#   scripts/worktree.sh gc    [--all] [--dry-run]    # purge incremental/ bloat (kept worktrees)
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

# Emit (to stdout, eval-able) the shared-compilation-cache env so EVERY worktree
# reuses compiled artifacts despite separate target/ dirs — this is what turns
# N concurrent worktrees from N× cold compiles into ~1× compile cost. Opt-in:
# only emitted when sccache is installed, so machines/CI without it are
# unaffected (CI keeps Swatinem/rust-cache). RUSTC_WRAPPER does not conflict
# with the repo's .cargo/config.toml rustflags; CARGO_INCREMENTAL=0 is required
# for sccache to cache at all.
emit_cache_env() {
  if command -v sccache >/dev/null 2>&1; then
    printf 'export RUSTC_WRAPPER=%q\n' "$(command -v sccache)"
    printf 'export SCCACHE_DIR=%q\n' "${SCCACHE_DIR:-$HOME/.cache/sccache}"
    # LRU-capped shared cache (sccache evicts past this), so disk stays bounded
    # unlike the unbounded per-worktree incremental/ dirs it replaces. Default
    # 25G suits the ~66-crate workspace's hit rate; override with SCCACHE_CACHE_SIZE.
    printf 'export SCCACHE_CACHE_SIZE=%q\n' "${SCCACHE_CACHE_SIZE:-25G}"
    printf 'export CARGO_INCREMENTAL=0\n'
  elif [ "${PROXIMADB_KEEP_INCREMENTAL:-0}" = "1" ]; then
    printf 'worktree: sccache absent and PROXIMADB_KEEP_INCREMENTAL=1 — incremental ON; expect multi-GB target/ growth per worktree. Reclaim it with: scripts/worktree.sh gc\n' >&2
  else
    # No sccache: disable incremental anyway. Per-worktree target/*/incremental
    # dirs (multi-GB dep-graph.bin/query-cache.bin per crate) are the dominant
    # RECURRING disk consumer and are never shared across worktrees, so in this
    # many-worktree workflow their disk cost outweighs their rebuild-speed
    # benefit. CARGO_INCREMENTAL=0 stops them being created. For fast rebuilds,
    # install sccache (shared cross-worktree cache) — then this branch is unused.
    printf 'export CARGO_INCREMENTAL=0\n'
    printf 'worktree: incremental compilation OFF to avoid multi-GB per-worktree target/ bloat. For fast rebuilds install a shared cache: brew install sccache (or cargo install sccache). Override with PROXIMADB_KEEP_INCREMENTAL=1.\n' >&2
  fi
}

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
  # Activate the repo's fast pre-push hook (idempotent + repo-wide; each
  # worktree resolves .githooks/ from its own checkout).
  git -C "$(repo_main)" config core.hooksPath .githooks 2>/dev/null || true
  # Emit a `cd` + the shared-cache env so `eval "$(... new ...)"` lands in the
  # worktree AND wires up sccache for fast, dedup'd compilation across worktrees.
  printf 'cd %q\n' "$dir"
  emit_cache_env
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

# Is <branch> already in develop? Catches BOTH integration styles:
#   1. fast-forward / merge-commit — branch tip is an ancestor of develop.
#   2. squash-merge (GitHub default) — branch commits are rewritten into ONE
#      commit with a new SHA, so the tip is NOT an ancestor. We detect this the
#      git-delete-squashed idiom: synthesize a single commit of the branch's
#      tree on top of the merge-base and ask `git cherry` whether develop already
#      contains an equivalent patch (a leading '-' means yes). Offline-safe — no
#      remote branch required, only the local develop ref.
branch_in_develop() {
  local main br; main="$(repo_main)"; br="$1"
  git -C "$main" merge-base --is-ancestor "$br" "$REMOTE/$BASE_DEFAULT" 2>/dev/null && return 0
  local mb tree synth; mb="$(git -C "$main" merge-base "$REMOTE/$BASE_DEFAULT" "$br" 2>/dev/null)" || return 1
  [ -n "$mb" ] || return 1
  tree="$(git -C "$main" rev-parse "$br^{tree}" 2>/dev/null)" || return 1
  synth="$(git -C "$main" commit-tree "$tree" -p "$mb" -m _ 2>/dev/null)" || return 1
  [ "$(git -C "$main" cherry "$REMOTE/$BASE_DEFAULT" "$synth" 2>/dev/null | head -1 | cut -c1)" = "-" ]
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

# Drop every worktree whose branch has landed in develop (merge OR squash),
# reclaiming its target/ build cache — the dominant disk consumer (tens of GB
# per worktree). Dirty worktrees are always skipped. `--dry-run` reports what
# WOULD be reclaimed without touching anything.
cmd_clean() {
  local dry=""; [ "${1:-}" = "--dry-run" ] || [ "${1:-}" = "-n" ] && dry=1
  git -C "$(repo_main)" fetch --quiet "$REMOTE" "$BASE_DEFAULT" || true
  local removed=0 freed_kb=0
  while read -r dir; do
    [ "$dir" = "$(repo_main)" ] && continue
    local br; br="$(git -C "$dir" rev-parse --abbrev-ref HEAD 2>/dev/null || echo)"
    [ -n "$br" ] || continue
    branch_in_develop "$br" || continue
    if [ -n "$(git -C "$dir" status --porcelain 2>/dev/null)" ]; then
      printf 'skip (dirty): %s (%s)\n' "$dir" "$br" >&2; continue
    fi
    local kb; kb="$(du -sk "$dir" 2>/dev/null | cut -f1)"; kb="${kb:-0}"
    freed_kb=$((freed_kb + kb))
    if [ -n "$dry" ]; then
      printf 'would clean: %s (%s) — %s MB\n' "$dir" "$br" "$((kb/1024))" >&2
    else
      git -C "$(repo_main)" worktree remove "$dir" && git -C "$(repo_main)" branch -D "$br" >/dev/null 2>&1 || true
      printf 'cleaned merged worktree: %s (%s) — reclaimed %s MB\n' "$dir" "$br" "$((kb/1024))" >&2
    fi
    removed=$((removed+1))
  done < <(git -C "$(repo_main)" worktree list --porcelain | sed -n 's/^worktree //p')
  [ -n "$dry" ] || git -C "$(repo_main)" worktree prune
  local verb="cleaned" gerund="reclaimed"
  [ -n "$dry" ] && { verb="would clean"; gerund="reclaiming"; }
  printf 'worktree: %s %d merged worktree(s), %s %s GB\n' \
    "$verb" "$removed" "$gerund" \
    "$(awk -v k="$freed_kb" 'BEGIN{printf "%.1f", k/1024/1024}')" >&2
}

# Reclaim build-cache bloat from worktrees you KEEP — deletes the regenerable
# target/*/incremental dirs (the dominant recurring consumer: multi-GB
# dep-graph.bin/query-cache.bin per crate) without removing the worktree or
# touching source/WIP. Defaults to the CURRENT worktree only (safe — your own
# session). `--all` sweeps every worktree; cargo just rebuilds the incremental
# state next compile, but if another session has a build IN FLIGHT there it will
# be forced to recompile, so --all warns. `--dry-run` previews. The lasting fix
# is to stop creating these (sccache / CARGO_INCREMENTAL=0 via `new`/`cache-env`).
cmd_gc() {
  local all="" dry="" dirs
  for a in "$@"; do
    case "$a" in --all) all=1;; --dry-run|-n) dry=1;; esac
  done
  if [ -n "$all" ]; then
    [ -n "$dry" ] || printf 'worktree: gc --all — purging incremental dirs in ALL worktrees; an in-flight build elsewhere will recompile\n' >&2
    dirs="$(git -C "$(repo_main)" worktree list --porcelain | sed -n 's/^worktree //p')"
  else
    dirs="$(git rev-parse --show-toplevel 2>/dev/null || echo)"
    [ -n "$dirs" ] || die "not in a git repo — cd into a worktree, or pass --all"
  fi
  local freed_kb=0
  while read -r d; do
    [ -d "$d/target" ] || continue
    while read -r inc; do
      [ -d "$inc" ] || continue
      local kb; kb="$(du -sk "$inc" 2>/dev/null | cut -f1)"; kb="${kb:-0}"
      freed_kb=$((freed_kb + kb))
      if [ -n "$dry" ]; then
        printf 'would gc: %s — %s MB\n' "$inc" "$((kb/1024))" >&2
      else
        rm -rf "$inc" && printf 'gc: removed %s — %s MB\n' "$inc" "$((kb/1024))" >&2
      fi
    done < <(find "$d/target" -type d -name incremental -prune 2>/dev/null)
  done <<< "$dirs"
  local verb="reclaimed"; [ -n "$dry" ] && verb="would reclaim"
  printf 'worktree: gc %s %s GB of incremental build cache\n' \
    "$verb" "$(awk -v k="$freed_kb" 'BEGIN{printf "%.1f", k/1024/1024}')" >&2
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

# Affected-only feedback: build/test ONLY the crates whose source changed vs
# develop (mirrors CI change-detection locally), so an agent touching one crate
# doesn't compile the whole 66-crate workspace. Direct crates only — for a
# low-level/foundation change, run the full `make test` too.
_affected() { python3 "$(repo_main)/scripts/affected_crates.py" "$REMOTE/$BASE_DEFAULT"; }
_pkg_args() { local a=""; for p in $1; do a="$a -p $p"; done; printf '%s' "$a"; }

cmd_check() {
  local pkgs; pkgs="$(_affected)"
  [ -n "$pkgs" ] || { printf 'worktree: no crate source changed vs %s — nothing to check\n' "$BASE_DEFAULT" >&2; return 0; }
  # Two complementary checks that together match CI without false reds:
  #  1. `cargo check --tests` COMPILES the lib/bin `#[cfg(test)]` code + integration
  #     tests — the "green local, red CI" trap fix: a plain `cargo check` or
  #     `clippy --lib --bins` never compiles the test cfg, so a broken/changed
  #     import inside a test module sails through locally and only fails in CI's
  #     "Rust Tests" job. Scoped to `--tests` (NOT `--all-targets`): CI's Rust
  #     Tests job runs `--lib` + doctests and never builds benches/examples, which
  #     have bit-rotted on the root crate; `--all-targets` would flag that
  #     pre-existing rot as a false red. (No `-D warnings` on test code — the root
  #     crate's tests carry many pre-existing clippy warnings CI does not gate.)
  #  2. `clippy --lib --bins -- -D warnings` lints exactly what CI's clippy gate
  #     lints (lib+bins only), so the lint posture matches CI precisely.
  printf 'worktree: cargo check --tests%s (compiles #[cfg(test)] — CI Rust Tests gap)\n' "$(_pkg_args "$pkgs")" >&2
  # shellcheck disable=SC2046
  cargo check $(_pkg_args "$pkgs") --tests
  printf 'worktree: cargo clippy --lib --bins -D warnings%s (matches CI clippy gate)\n' "$(_pkg_args "$pkgs")" >&2
  # shellcheck disable=SC2046
  cargo clippy $(_pkg_args "$pkgs") --lib --bins -- -D warnings
}

cmd_test() {
  local pkgs; pkgs="$(_affected)"
  [ -n "$pkgs" ] || { printf 'worktree: no crate source changed vs %s — nothing to test\n' "$BASE_DEFAULT" >&2; return 0; }
  # Mirror CI's "Rust Tests" job EXACTLY so green-local == green-CI:
  #   unit: cargo nextest run --lib --profile unit --test-threads=2
  #   doc : cargo test --doc -- --test-threads=4
  # `--test-threads` bounds the global-statics races the root suite is known to
  # have (WAL registry / metadata provider / request-id counter) — running
  # unbounded locally drifts from CI and hides (or invents) flakes. `--profile
  # unit` applies the nextest retry/config CI uses. `--lib` scopes to unit tests
  # like CI. (CARGO_BUILD_JOBS stays at the local default — JOBS=1 is a 16GB-CI
  # OOM workaround, not needed on a dev box.)
  printf 'worktree: cargo nextest run --lib --profile unit --test-threads=2%s (matches CI)\n' "$(_pkg_args "$pkgs")" >&2
  # shellcheck disable=SC2046
  cargo nextest run --lib --profile unit --test-threads=2 $(_pkg_args "$pkgs")
  printf 'worktree: cargo test --doc --test-threads=4%s (matches CI doc tests)\n' "$(_pkg_args "$pkgs")" >&2
  # shellcheck disable=SC2046
  cargo test --doc $(_pkg_args "$pkgs") -- --test-threads=4
}

case "${1:-}" in
  new)   shift; cmd_new "$@" ;;
  list)  shift; cmd_list "$@" ;;
  rm)    shift; cmd_rm "$@" ;;
  clean) shift; cmd_clean "$@" ;;
  gc)    shift; cmd_gc "$@" ;;
  guard) shift; cmd_guard "$@" ;;
  cache-env) shift; emit_cache_env ;;
  check) shift; cmd_check "$@" ;;
  test)  shift; cmd_test "$@" ;;
  *) cat >&2 <<'USAGE'
worktree: one task = one worktree = one branch (isolated by construction)
  scripts/worktree.sh new   <type/topic> [base]   create + print `cd` + cache env (eval it)
  scripts/worktree.sh list                          list worktrees + dirty state
  scripts/worktree.sh rm    <type/topic> [--force]  remove (guards dirty)
  scripts/worktree.sh clean [--dry-run]             drop worktrees merged/squashed to develop, reclaim target/
  scripts/worktree.sh gc [--all] [--dry-run]        purge target/*/incremental bloat from KEPT worktree(s)
  scripts/worktree.sh guard                         fail if run in the main checkout
  scripts/worktree.sh cache-env                     print sccache env for an existing worktree (eval it)
  scripts/worktree.sh check                         cargo check ONLY the crates changed vs develop
  scripts/worktree.sh test                          cargo nextest ONLY the crates changed vs develop
USAGE
     exit 2 ;;
esac
