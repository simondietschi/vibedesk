#!/usr/bin/env bash
# Holt die aktuellen Upstream-Stände von rustdesk und rustdesk-server
# (inkl. der jeweils fixierten hbb_common-Version) per git subtree.
#
# Aufruf: scripts/sync-upstream.sh [client|server|all]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

HBB_COMMON=https://github.com/rustdesk/hbb_common

if [ -n "$(git status --porcelain)" ]; then
  echo "Arbeitsverzeichnis ist nicht sauber, bitte zuerst committen." >&2
  exit 1
fi

sync() {
  local prefix=$1 repo=$2 branch=$3

  echo "== $prefix <- $repo ($branch)"
  git fetch -q "$repo" "$branch"
  local upstream hbb current_hbb
  upstream=$(git rev-parse FETCH_HEAD)
  hbb=$(git ls-tree "$upstream" libs/hbb_common | awk '{print $3}')

  git subtree pull --prefix="$prefix" "$repo" "$branch" --squash \
    -m "Sync $prefix with upstream ${upstream:0:9}"

  # Upstream fuehrt hbb_common als Submodule; hier ist es ein Subtree.
  # Der zuletzt importierte Stand steht in der Squash-Commit-Nachricht.
  current_hbb=$(git log -1 --grep="^git-subtree-dir: $prefix/libs/hbb_common\$" \
    --format=%b | sed -n 's/^git-subtree-split: //p')
  if [ -z "$hbb" ]; then
    echo "   hbb_common nicht im Upstream gefunden, uebersprungen."
  elif [ "$hbb" = "$current_hbb" ]; then
    echo "   hbb_common unveraendert (${hbb:0:9})."
  else
    git fetch -q "$HBB_COMMON" "$hbb"
    git subtree pull --prefix="$prefix/libs/hbb_common" "$HBB_COMMON" "$hbb" --squash \
      -m "Sync $prefix/libs/hbb_common with upstream ${hbb:0:9}"
  fi
}

case "${1:-all}" in
  client) sync client https://github.com/rustdesk/rustdesk master ;;
  server) sync server https://github.com/rustdesk/rustdesk-server master ;;
  all)
    sync client https://github.com/rustdesk/rustdesk master
    sync server https://github.com/rustdesk/rustdesk-server master
    ;;
  *) echo "Aufruf: $0 [client|server|all]" >&2; exit 2 ;;
esac
