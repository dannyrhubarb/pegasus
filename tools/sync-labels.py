#!/usr/bin/env python3
"""Sync the repository's GitHub issue labels from .github/labels.json.

The JSON file is the source of truth for the label convention (see
CONTRIBUTING.md → "Issue labels"): each entry is
{"name", "color", "description", "aliases"?}. For every entry the script
creates the label, or updates its color/description when it exists, or
RENAMES a label listed under "aliases" (renaming keeps the label attached
to every issue that already carries it — that is how the GitHub defaults
`bug`/`enhancement`/… were folded into the `type:` group). With --prune,
labels that exist on the repo but not in the file are deleted; without it
they are only reported.

Runs in CI (.github/workflows/labels.yml, on every change to the file) and
by hand:

    GITHUB_TOKEN=… python3 tools/sync-labels.py --repo owner/name [--prune]

Standard library only — no PyYAML, no gh CLI.
"""
import argparse
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.github.com"


def request(token, method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(API + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    req.add_header("Accept", "application/vnd.github+json")
    req.add_header("X-GitHub-Api-Version", "2022-11-28")
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")[:300]
        sys.exit(f"{method} {path} -> HTTP {e.code}: {detail}")


def existing_labels(token, repo):
    labels = {}
    page = 1
    while True:
        batch = request(token, "GET", f"/repos/{repo}/labels?per_page=100&page={page}")
        if not batch:
            return labels
        for label in batch:
            labels[label["name"]] = label
        page += 1


def quote(name):
    return urllib.parse.quote(name, safe="")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"),
                    help="owner/name (default: $GITHUB_REPOSITORY)")
    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ap.add_argument("--file", default=os.path.join(repo_root, ".github", "labels.json"),
                    help="label definitions (default: this repo's .github/labels.json)")
    ap.add_argument("--prune", action="store_true",
                    help="delete labels that are not in the file")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not args.repo or not token:
        sys.exit("need --repo (or $GITHUB_REPOSITORY) and $GITHUB_TOKEN")

    with open(args.file, encoding="utf-8") as fh:
        wanted = json.load(fh)
    names = [w["name"] for w in wanted]
    if len(names) != len(set(names)):
        sys.exit("duplicate label names in " + args.file)
    for w in wanted:
        if not (isinstance(w.get("color"), str) and len(w["color"]) == 6
                and all(c in "0123456789abcdefABCDEF" for c in w["color"])):
            sys.exit(f"{w.get('name')!r}: color must be 6 hex digits, no #")
        if len(w.get("description", "")) > 100:
            sys.exit(f"{w['name']!r}: description over GitHub's 100-char cap")

    current = existing_labels(token, args.repo)
    changed = 0

    def act(verb, label, path, method, body):
        nonlocal changed
        changed += 1
        print(f"{verb:7} {label}")
        if not args.dry_run:
            request(token, method, path, body)

    for w in wanted:
        payload = {"new_name": w["name"], "color": w["color"].lower(),
                   "description": w.get("description", "")}
        if w["name"] in current:
            have = current[w["name"]]
            if (have["color"].lower() != payload["color"]
                    or (have.get("description") or "") != payload["description"]):
                act("update", w["name"], f"/repos/{args.repo}/labels/{quote(w['name'])}",
                    "PATCH", payload)
            continue
        alias = next((a for a in w.get("aliases", []) if a in current), None)
        if alias:
            act("rename", f"{alias} -> {w['name']}",
                f"/repos/{args.repo}/labels/{quote(alias)}", "PATCH", payload)
            current.pop(alias)
            continue
        act("create", w["name"], f"/repos/{args.repo}/labels", "POST",
            {"name": w["name"], "color": payload["color"],
             "description": payload["description"]})

    aliased = {a for w in wanted for a in w.get("aliases", [])}
    extra = [n for n in current if n not in names and n not in aliased]
    for name in extra:
        if args.prune:
            act("delete", name, f"/repos/{args.repo}/labels/{quote(name)}", "DELETE", None)
        else:
            print(f"extra   {name} (not in file; --prune deletes it)")
    print(f"{changed} change(s){' (dry run)' if args.dry_run else ''}")


if __name__ == "__main__":
    main()
