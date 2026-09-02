#!/usr/bin/env python3
"""Assert the website and both app bundles copy the same files.

Three places independently hand-maintain the list of files that make up a
Pegasus build:

    .github/actions/build-site/action.yml   the website
    ios/sync-web.sh                         the iOS app's WebRoot
    android/sync-web.sh                     the Android app's webroot

Nothing tied them together, so adding a file to the website and forgetting
the two shells was a silent failure: the site gets it, both apps quietly
ship without it, and it only surfaces as a broken link on a device. Running
sync-web.sh in CI does NOT catch this — the script still exits 0, it just
copies one file fewer. Only comparing the lists catches it, which is what
this does.

Run with no arguments; exits non-zero and names the offending file(s).
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Files the WEBSITE ships that the apps deliberately do not. Every entry
# needs a reason — this set is the whole point of the check, so growing it
# without one defeats it.
WEB_ONLY = {
    # Generated at deploy time by rsvg-convert from icon.svg, and only
    # meaningful to a browser: the apps carry real native launcher icons,
    # and neither WKWebView nor Android WebView consumes apple-touch-icon
    # or the web manifest's icon list. Generating them in the sync scripts
    # would make librsvg a prerequisite for building the apps, for nothing.
    "icon-512.png",
    "icon-192.png",
    "icon-180.png",
    # The level editor is deliberately UNLINKED from the game UI (no menu
    # button, no picker row) and is reachable only by opening its own URL,
    # which cannot happen inside an app shell. 95 KB of unreachable page.
    "editor.html",
    # The update/config policy (#190) is served to be FETCHED LIVE at
    # launch — the shells from the live site root, the web page relative.
    # Bundling it into an app would freeze exactly the thing that must
    # stay remote (see "App update policy" in CLAUDE.md).
    "app-policy.json",
    # Digital Asset Links for the Play app (assetlinks.json): Android and
    # Chrome fetch it from the LIVE domain to verify App Links / the
    # related-app install prompt. Nothing inside a WKWebView/WebView
    # shell ever requests it, and a copy on an app-local origin would
    # verify nothing anyway.
    ".well-known",
}

# Differences in HOW a file is produced, not IN WHICH bundle it appears,
# are out of scope here and documented in the sync scripts themselves:
# version.json (web only — the stale-cache toast is meaningless in-app),
# config.json (deploy variable vs fetched from the live site) and the
# -ios/-android revision suffix.

CP = re.compile(r"^\s*cp\s+(?P<rest>.+?)\s*$")


def sources(path: Path) -> set[str]:
    """Every REPO-FILE source operand of every `cp` in the file.

    Operands containing `$` are computed paths (the wasm-opt fallback's
    "$WASM_SRC", destinations like "$DEST/") rather than files checked into
    the repo, so they are not part of the comparison.
    """
    found: set[str] = set()
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        m = CP.match(line)
        if not m:
            continue
        tokens = m.group("rest").split()
        operands = [t for t in tokens if not t.startswith("-")]  # drop flags
        if len(operands) < 2:
            continue
        # Order matters: drop the DESTINATION first (always the last
        # operand, and usually "$DEST/"), and only then discard computed
        # sources. Filtering on `$` first would eat the destination and
        # leave the last real source looking like one.
        found.update(t for t in operands[:-1] if "$" not in t)
    if not found:
        raise SystemExit(f"{path}: no cp lines found — has the file moved?")
    return found


def main() -> int:
    site = sources(ROOT / ".github/actions/build-site/action.yml")
    ios = sources(ROOT / "ios/sync-web.sh")
    android = sources(ROOT / "android/sync-web.sh")

    problems: list[str] = []

    # The two shells bundle the same web build; they must not diverge at all.
    if ios != android:
        for f in sorted(ios ^ android):
            where = "ios" if f in ios else "android"
            problems.append(
                f"{f}: copied by {where}/sync-web.sh only — the two app "
                f"bundles must be identical"
            )

    expected = site - WEB_ONLY
    for f in sorted(expected - ios - android):
        problems.append(
            f"{f}: shipped to the website by build-site but MISSING from the "
            f"app bundles — add it to ios/sync-web.sh and android/sync-web.sh, "
            f"or add it to WEB_ONLY here with a reason"
        )
    for f in sorted((ios | android) - site):
        problems.append(
            f"{f}: bundled into the apps but not shipped to the website by "
            f"build-site — one of the three lists is wrong"
        )
    for f in sorted(WEB_ONLY & (ios | android)):
        problems.append(
            f"{f}: listed as WEB_ONLY here but the apps do bundle it — drop "
            f"it from WEB_ONLY"
        )
    for f in sorted(WEB_ONLY - site):
        problems.append(
            f"{f}: listed as WEB_ONLY here but build-site no longer ships it "
            f"— drop it from WEB_ONLY"
        )

    if problems:
        print("Bundle file lists are out of sync:\n", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        print(
            "\nThe website and both apps are assembled from three separate "
            "hand-maintained lists;\nthey have to be changed together. See "
            "tools/check-bundle-sync.py.",
            file=sys.stderr,
        )
        return 1

    print(f"Bundle lists in sync ({len(expected)} shared files, "
          f"{len(WEB_ONLY)} web-only).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
