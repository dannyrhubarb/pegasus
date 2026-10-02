#!/usr/bin/env sh
# Propose the NEXT release tag from the current one (see CLAUDE.md
# "Versioning" — the Cut release workflow's number step):
#
#   tools/next-version.sh patch    → v1.0.2   (from v1.0.1)
#   tools/next-version.sh minor    → v1.1.0
#   tools/next-version.sh major    → v2.0.0
#
# The base is the nearest `vX.Y.Z` tag reaching HEAD, exactly as
# tools/version.sh resolves it (so the two never disagree); with no tag
# anywhere the base is 0.0.0 and `minor` proposes v0.1.0. Pure: prints
# the tag and nothing else, touches nothing. Needs full history + tags.
set -eu
bump="${1:-}"
case "$bump" in major|minor|patch) ;; *)
  echo "usage: $0 major|minor|patch" >&2; exit 2 ;;
esac
base="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || echo v0.0.0)"
base="${base#v}"
major="${base%%.*}"; rest="${base#*.}"
minor="${rest%%.*}"; patch="${rest#*.}"
case "$bump" in
  major) major=$((major + 1)); minor=0; patch=0 ;;
  minor) minor=$((minor + 1)); patch=0 ;;
  patch) patch=$((patch + 1)) ;;
esac
echo "v${major}.${minor}.${patch}"
