#!/usr/bin/env bash
# Shared by bench/owner_stalls_ab.sh and bench/commits_ab.sh (kiln goal ab3aedaf: each record against the commit
# before it): a commit's own worktree and target directory, one per tree, so two commits of one tree build once
# and a build never takes another tree's crates as fresh. Sourced, not run.
# ab_tree <commit>: the commit's tree hash. ab_worktree <commit>: prints its worktree, made once.
# ab_load: the host's one-minute load, or "unknown" where the host doesn't say.
AB_WORK="${AB_WORK:-$HOME/.cache/amphora-ab}"
ab_tree() { git rev-parse "$1^{tree}"; }
ab_worktree() {
  local tree dir
  tree=$(ab_tree "$1") || return 1
  dir="$AB_WORK/src-$tree"
  if [[ ! -d "$dir" ]]; then
    git worktree prune
    git worktree add -q --detach "$dir" "$1" >&2 || return 1
  fi
  echo "$dir"
}
ab_load() {
  if [[ -r /proc/loadavg ]]; then cut -d' ' -f1 /proc/loadavg
  elif command -v sysctl >/dev/null && sysctl -n vm.loadavg >/dev/null 2>&1; then sysctl -n vm.loadavg | awk '{print $2}'
  else echo unknown; fi
}
