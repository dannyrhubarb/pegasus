#!/usr/bin/env python3
"""Generate the What's New data (whats-new.json) for the About screen.

Merges two sources into one newest-first JSON array of
{"rev", "date", "note"} entries:

- git history: every reachable commit carrying a `Whats-new:` trailer —
  the per-commit maintenance rule for user-facing changes (see CLAUDE.md
  "What's new page"). Revision, date and time come from git itself, so
  they survive the rebase merges that rewrite branch shas. The date is
  the COMMITTER date — when the change landed on main, not when the
  branch work was started (see LOG_FORMAT below).
- tools/whats-new-backfill.json: hand-curated entries for user-facing
  commits that predate the trailer convention. Their main shas are
  final, so pinning them in a file is safe — but never add NEW entries
  here; new changes get a trailer instead.

Runs at deploy time in the build-site action, which is why the deploy
workflows check out with fetch-depth: 0 — a shallow clone silently loses
every trailered commit below the head. Also runnable locally:

    python3 tools/gen-whats-new.py > whats-new.json
"""

import json
import subprocess
import sys
from datetime import datetime
from pathlib import Path

BACKFILL = Path(__file__).with_name("whats-new-backfill.json")

# \x1f between fields, \x1e between commits: commit subjects/trailers can
# contain anything printable, so field-split on control characters.
# %cI = COMMITTER date: the rebase merge rewrites it to the moment the
# commit landed on main, so entries order by when players actually got
# the change. The author date (%aI) survives rebases and would sort a
# long-lived branch by when the work was STARTED — a change merged today
# could sink below entries authored hours after it (seen live 2026-07-13).
LOG_FORMAT = "%h%x1f%cI%x1f%(trailers:key=Whats-new,valueonly,separator=%x20,unfold)%x1e"


def trailer_entries():
    try:
        out = subprocess.run(
            ["git", "log", f"--format={LOG_FORMAT}"],
            capture_output=True, text=True, check=True,
        ).stdout
    except (subprocess.CalledProcessError, OSError) as e:
        # No git / no history: degrade to the backfill alone rather than
        # failing the whole deploy over the changelog.
        print(f"gen-whats-new: git log failed ({e}); backfill only", file=sys.stderr)
        return []
    entries = []
    for record in out.split("\x1e"):
        record = record.strip("\n")
        if not record:
            continue
        rev, date, note = record.split("\x1f")
        note = note.strip()
        if note:
            entries.append({"rev": rev, "date": date, "note": note})
    return entries


def sort_key(entry):
    # Author dates carry a UTC offset; compare as aware datetimes so mixed
    # offsets (+00:00 vs +02:00 both appear in history) order correctly.
    return datetime.fromisoformat(entry["date"].replace("Z", "+00:00"))


def main():
    entries = trailer_entries()
    seen = {e["rev"] for e in entries}
    for e in json.loads(BACKFILL.read_text(encoding="utf-8")):
        if e["rev"] not in seen:
            entries.append(e)
    entries.sort(key=sort_key, reverse=True)
    json.dump(entries, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
