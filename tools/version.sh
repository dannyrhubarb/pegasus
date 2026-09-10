#!/usr/bin/env sh
# The ONE source of version identity for every build (see CLAUDE.md
# "Versioning"): derived from git tags, never from a file anyone has to
# remember to bump.
#
#   tools/version.sh               → 1.3.0+14   (full: tag + commits since it)
#   tools/version.sh --marketing   → 1.3.0      (the tag alone — the store apps'
#                                                CFBundleShortVersionString /
#                                                versionName; fails when HEAD
#                                                has no vX.Y.Z tag ancestor —
#                                                a store release must be cut
#                                                from a tagged history)
#   tools/version.sh --commit-date → 2026-09-10T10:34:56+00:00 (HEAD's committer
#                                                date, UTC — the deterministic
#                                                stand-in for a build timestamp)
#
# Tags are annotated `vMAJOR.MINOR.PATCH` on main. Between tags the full form
# appends the commit distance (`+14`), so every push to main has a strictly
# increasing identity (tuple order: 1.3.0+14 < 1.3.1+0 < 1.4.0+2) with no
# manual bump. Before the first tag the base is 0.0.0 and the distance is the
# commit count, so the first tag (v1.0.0) sorts above every pre-tag build.
# Needs full history + tags (fetch-depth: 0 in CI fetches both).
set -eu
mode="${1:-full}"
desc="$(git describe --tags --long --match 'v[0-9]*' 2>/dev/null || true)"
if [ -n "$desc" ]; then
  # v1.3.0-14-gabc123 → base 1.3.0, ahead 14
  base="${desc#v}"; base="${base%-*-g*}"
  ahead="${desc%-g*}"; ahead="${ahead##*-}"
else
  base="0.0.0"
  ahead="$(git rev-list --count HEAD)"
fi
case "$mode" in
  full|"") echo "${base}+${ahead}" ;;
  --marketing)
    if [ -z "$desc" ]; then
      echo "version.sh: no vX.Y.Z tag reaches HEAD — tag a release first" \
           "(git tag -a v1.0.0 -m 'v1.0.0' && git push origin v1.0.0)" >&2
      exit 1
    fi
    echo "$base" ;;
  --commit-date) TZ=UTC git log -1 --date=iso-strict-local --format=%cd ;;
  *) echo "usage: $0 [--marketing|--commit-date]" >&2; exit 2 ;;
esac
