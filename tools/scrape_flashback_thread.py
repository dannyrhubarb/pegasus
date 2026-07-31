#!/usr/bin/env python3
"""Scrape every post from every page of a Flashback forum thread.

Stdlib-only (no pip installs). Fetches all pages of a thread like
https://www.flashback.org/t3739595, extracts each post (number, id, date,
author, message text incl. quoted posts) and writes JSON or plain text.

Usage:
    python3 scrape_flashback_thread.py https://www.flashback.org/t3739595
    python3 scrape_flashback_thread.py t3739595 -f text -o thread.txt
    python3 scrape_flashback_thread.py 3739595 --pages 1-5 -o - | jq .

    # top up an earlier scrape with only the posts made since
    python3 scrape_flashback_thread.py t3739595 --append -o t3739595.json

By default the output lands in t<thread-id>.json / .txt next to where you
run it; pass `-o -` for stdout. A polite delay (default 1 s) is kept
between page fetches.

--append reads the existing output file, resumes at the page holding the
last post it already has, and adds only posts whose id isn't in the file
yet, so re-running it costs a couple of page fetches instead of the whole
thread. Both output formats can be topped up (each post's permalink
carries its id). A post's body is kept as first captured -- later edits
and deletions don't rewrite what's already stored.

Three position-ish fields, because they answer different questions:
  post_id   stable identity, never reused; sequential, so id order is post
            order -- this is what the file is sorted and deduped by.
  index     1..N position within this archive, recomputed on every write,
            always contiguous. Use this to count or cite posts.
  number    the position the forum itself showed when the post was read.
            Deleting a post shifts every later post down a slot, so after
            an --append run the numbers around the resume point can repeat
            (older posts keep the numbers they were read with). Harmless,
            but don't treat `number` as a key. Each post's `url` is the
            exact cross-reference back to the live thread.
"""

import argparse
import html
import json
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timedelta, timezone

BASE = "https://www.flashback.org"
USER_AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/126.0 Safari/537.36"
)
# Flashback renders same-day/previous-day posts as "Idag"/"Igår" in the
# forum's own timezone, so relative dates are pinned against this.
FORUM_TZ = "Europe/Stockholm"
POSTS_PER_PAGE = 12  # only a fallback; the real page size is measured at runtime
RESUME_BACKOFF_PAGES = 3  # slack for posts that shifted pages under deletions


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def forum_tz():
    try:
        from zoneinfo import ZoneInfo
        return ZoneInfo(FORUM_TZ)
    except Exception:  # no tzdata on this box: CEST is close enough to pin a day
        return timezone(timedelta(hours=2))


def resolve_timestamp(raw, ref):
    """'2026-07-28, 23:32' | 'Idag, 00:15' | 'Igår, 23:50' -> '2026-07-28 23:32'.

    `ref` is the forum-local time the page was fetched, which is what the
    relative labels are relative to. Returns None if the shape is unknown.
    """
    if not raw:
        return None
    m = re.match(r"\s*(.+?)\s*,\s*(\d{1,2}):(\d{2})\s*$", raw)
    if not m:
        return None
    day, hh, mm = m.group(1).strip(), int(m.group(2)), int(m.group(3))
    if re.fullmatch(r"\d{4}-\d{2}-\d{2}", day):
        date = day
    elif day.lower() == "idag":
        date = ref.date().isoformat()
    elif day.lower() in ("igår", "igar"):
        date = (ref.date() - timedelta(days=1)).isoformat()
    else:
        return None
    return f"{date} {hh:02d}:{mm:02d}"


def fetch(url, retries=4):
    """GET a page and decode it (Flashback serves ISO-8859-1)."""
    last_err = None
    for attempt in range(retries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=30) as resp:
                raw = resp.read()
                ctype = resp.headers.get("Content-Type", "")
            m = re.search(r"charset=([\w-]+)", ctype)
            enc = m.group(1) if m else None
            if not enc:
                m = re.search(rb'charset="?([\w-]+)', raw[:2000])
                enc = m.group(1).decode("ascii") if m else "ISO-8859-1"
            return raw.decode(enc, errors="replace")
        except (urllib.error.URLError, OSError, TimeoutError) as e:
            last_err = e
            wait = 2 ** attempt
            log(f"  fetch failed ({e}), retrying in {wait}s ...")
            time.sleep(wait)
    raise RuntimeError(f"could not fetch {url}: {last_err}")


def thread_id_from(arg):
    m = re.search(r"t(\d+)", arg) or re.fullmatch(r"(\d+)", arg.strip())
    if not m:
        sys.exit(f"error: cannot find a thread id in {arg!r} "
                 "(expected e.g. https://www.flashback.org/t3739595)")
    return m.group(1)


def page_url(tid, page):
    return f"{BASE}/t{tid}" + (f"p{page}" if page > 1 else "")


def balanced_div(html_text, start):
    """Return the end index of the <div> opening at `start` (past its </div>)."""
    depth = 0
    for m in re.finditer(r"<div\b|</div\s*>", html_text[start:]):
        depth += 1 if m.group(0).startswith("<div") else -1
        if depth == 0:
            return start + m.end()
    return len(html_text)


def html_to_text(fragment):
    """Flatten a post_message HTML fragment to readable plain text."""
    s = fragment
    s = re.sub(r"<br\s*/?>", "\n", s, flags=re.I)
    # Quote blocks: keep them readable as indented text.
    s = re.sub(r"<small>\s*Citat:\s*</small>", "\nCitat:\n", s, flags=re.I)
    # Block-level boundaries become newlines, all other tags vanish.
    s = re.sub(r"</?(div|p|blockquote|ul|ol|li|h\d)[^>]*>", "\n", s, flags=re.I)
    s = re.sub(r"<[^>]+>", "", s)
    s = html.unescape(s)
    lines = [re.sub(r"[ \t\xa0]+", " ", ln).strip() for ln in s.split("\n")]
    out, blank = [], True
    for ln in lines:
        if ln:
            out.append(ln)
            blank = False
        elif not blank:
            out.append("")
            blank = True
    while out and not out[-1]:
        out.pop()
    return "\n".join(out)


def parse_page(page_html, page_no, fetched_at=None):
    fetched_at = fetched_at or datetime.now(forum_tz())
    posts = []
    for m in re.finditer(r'<div id="post(\d+)" data-postid="\1"', page_html):
        pid = m.group(1)
        block = page_html[m.start():balanced_div(page_html, m.start())]

        num = re.search(rf'id="postcount{pid}" name="(\d+)"', block)
        date = re.search(
            r"(\d{4}-\d{2}-\d{2}|Idag|Ig&aring;r|Igår)\s*,\s*(\d{2}:\d{2})", block)
        user = re.search(
            r'class="post-user-username[^"]*"[^>]*href="/u(\d+)"\s*>\s*([^<]+?)\s*</a>',
            block, re.S)
        msg = re.search(rf'<div class="post_message" id="post_message_{pid}">', block)
        if msg:
            body = block[msg.end():balanced_div(block, msg.start())]
            body = body[: body.rfind("</div")]
        else:
            body = ""

        raw_date = html.unescape(f"{date.group(1)}, {date.group(2)}") if date else None
        posts.append({
            "page": page_no,
            "number": int(num.group(1)) if num else None,
            "post_id": pid,
            "date": raw_date,
            "timestamp": resolve_timestamp(raw_date, fetched_at),
            "username": html.unescape(user.group(2)) if user else None,
            "user_id": user.group(1) if user else None,
            "url": f"{BASE}/sp{pid}",
            "message": html_to_text(body),
        })
    return posts


def as_text(meta, posts):
    sep = "-" * 72
    out = [meta["title"] or f"Thread {meta['thread_id']}",
           meta["url"],
           f"{len(posts)} posts on {meta['pages']} pages "
           f"(scraped {meta['scraped_at']})",
           sep]
    for p in posts:
        who = p["username"] or "?"
        when = p.get("timestamp") or p.get("date")
        n = p.get("index") or p.get("number")
        out.append(f"#{n} | {who} (u{p['user_id']}) | {when} | {p['url']}")
        out.append("")
        out.append(p["message"])
        out.append(sep)
    return "\n".join(out) + "\n"


def load_existing(path, fmt):
    """Read a previous run's output -> (meta, posts, known_ids).

    JSON keeps the full records. A text file only has to give us enough to
    resume and dedupe, so its posts come back as stubs (ids + last number)
    and its body is rewritten from scratch around the new posts.
    """
    try:
        with open(path, encoding="utf-8") as f:
            blob = f.read()
    except FileNotFoundError:
        return None, [], set()

    if fmt == "json":
        data = json.loads(blob)
        posts = data.pop("posts", [])
        return data, posts, {p["post_id"] for p in posts}

    # text: every post header carries its permalink, so ids are recoverable
    ids = re.findall(r"/sp(\d+)", blob)
    nums = [int(n) for n in re.findall(r"^#(\d+) \|", blob, re.M)]
    meta = {"post_count": len(ids), "last_number": max(nums) if nums else 0}
    return meta, [], set(ids)


def resume_page(existing_meta, existing_posts, known_ids, per_page):
    """First page worth re-fetching: the one holding the last known post.

    Backed off a couple of pages because a deleted post shifts every later
    post down a slot, which can move posts we already have onto an earlier
    page than we last saw them. Re-reading a page is free (ids are deduped
    and positions refreshed); missing one would leave a hole.
    """
    if existing_posts:
        last = max(p.get("page") or 0 for p in existing_posts)
    else:
        last_num = (existing_meta or {}).get("last_number") or len(known_ids)
        last = ((max(last_num, 1) - 1) // max(per_page, 1)) + 1
    return max(1, last - RESUME_BACKOFF_PAGES)


def main():
    ap = argparse.ArgumentParser(
        description="Scrape all posts from a Flashback forum thread.")
    ap.add_argument("thread", help="thread URL or id, e.g. "
                    "https://www.flashback.org/t3739595 or t3739595")
    ap.add_argument("-f", "--format", choices=["json", "text"], default=None,
                    help="output format (default: json, or inferred from -o suffix)")
    ap.add_argument("-o", "--output", default=None,
                    help="output file, '-' for stdout (default: t<id>.json/.txt)")
    ap.add_argument("--pages", default=None, metavar="A-B",
                    help="only scrape this page range, e.g. 3-10 or 5")
    ap.add_argument("--append", action="store_true",
                    help="update an existing output file in place: resume near "
                         "its last post and add only posts it doesn't have")
    ap.add_argument("--delay", type=float, default=1.0,
                    help="seconds to wait between page fetches (default 1.0)")
    args = ap.parse_args()

    if args.append and args.pages:
        sys.exit("error: --append picks its own page range; drop --pages")
    if args.append and args.output == "-":
        sys.exit("error: --append needs a real file to update, not stdout")

    tid = thread_id_from(args.thread)
    fmt = args.format
    if fmt is None and args.output and args.output.endswith(".txt"):
        fmt = "text"
    fmt = fmt or "json"
    out_path = args.output or f"t{tid}.{'txt' if fmt == 'text' else 'json'}"

    old_meta, old_posts, known_ids = (None, [], set())
    if args.append:
        old_meta, old_posts, known_ids = load_existing(out_path, fmt)
        if old_meta is None:
            log(f"{out_path} not found -- scraping the whole thread")
        else:
            log(f"{out_path}: {len(known_ids)} posts already stored")

    log(f"fetching {page_url(tid, 1)} ...")
    fetched_at = datetime.now(forum_tz())
    first = fetch(page_url(tid, 1))

    m = re.search(r"<title>(.*?)</title>", first, re.S)
    title = html.unescape(m.group(1)).replace(" - Flashback Forum", "").strip() if m else None
    m = re.search(r"Sidan\s+\d+\s+av\s+(\d+)", first)
    total = int(m.group(1)) if m else 1

    lo, hi = 1, total
    if args.append and known_ids:
        per_page = len(parse_page(first, 1, fetched_at)) or POSTS_PER_PAGE
        lo = min(resume_page(old_meta, old_posts, known_ids, per_page), total)
    if args.pages:
        m = re.fullmatch(r"(\d+)(?:-(\d+))?", args.pages.strip())
        if not m:
            sys.exit(f"error: bad --pages value {args.pages!r}")
        lo = int(m.group(1))
        hi = int(m.group(2) or m.group(1))
        hi = min(hi, total)
        if lo > hi:
            sys.exit(f"error: page range {lo}-{hi} is empty (thread has {total} pages)")

    log(f"'{title}' — {total} pages, scraping {lo}..{hi}")

    old_by_id = {p["post_id"]: p for p in old_posts}
    posts = []
    for page in range(lo, hi + 1):
        page_html = first if page == 1 else None
        if page_html is None:
            time.sleep(args.delay)
            page_html = fetch(page_url(tid, page))
        got = parse_page(page_html, page, fetched_at)
        fresh = []
        for p in got:
            old = old_by_id.get(p["post_id"])
            if old is not None:
                # Seen again on a re-fetched page: its position may have moved
                # (a deleted post shifts everything after it down a slot), so
                # take the current number/page. The body stays as captured.
                old["number"], old["page"] = p["number"], p["page"]
            elif p["post_id"] not in known_ids:
                fresh.append(p)
        known_ids.update(p["post_id"] for p in fresh)
        posts.extend(fresh)
        seen = f" ({len(got) - len(fresh)} already stored)" if len(fresh) != len(got) else ""
        log(f"  page {page}/{hi}: {len(fresh)} new{seen} (total {len(posts)})")

    added = len(posts)
    if old_posts:
        # Backfill relative dates stored by an earlier run ("Idag" meant that
        # run's today), then merge. Existing records win on conflict.
        ref = old_meta.get("scraped_at") if old_meta else None
        if ref:
            try:
                ref = datetime.fromisoformat(ref).astimezone(forum_tz())
            except ValueError:
                ref = None
        for p in old_posts:
            if not p.get("timestamp"):
                p["timestamp"] = resolve_timestamp(p.get("date"), ref) if ref else None
        # Sorted by post id, not by number: ids are globally sequential (so id
        # order is post order), while a post's number is only its position in
        # the thread at the time it was read and shifts under deletions.
        posts = old_posts + posts
        posts.sort(key=lambda p: int(p["post_id"]))
    # Appending to a text file: its posts came back as ids only, so continue
    # the numbering after them instead of restarting at 1.
    base = len(known_ids) - added if (args.append and not old_posts and known_ids) else 0
    for i, p in enumerate(posts, base + 1):
        p["index"] = i  # contiguous position in THIS archive, unlike `number`

    meta = {
        "thread_id": tid,
        "url": f"{BASE}/t{tid}",
        "title": title,
        "pages": total,
        "scraped_pages": [lo, hi],
        "scraped_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "post_count": len(posts),
    }
    if old_meta and old_meta.get("first_scraped_at"):
        meta["first_scraped_at"] = old_meta["first_scraped_at"]
    elif old_meta and old_meta.get("scraped_at"):
        meta["first_scraped_at"] = old_meta["scraped_at"]

    if fmt == "json":
        payload = json.dumps({**meta, "posts": posts}, ensure_ascii=False, indent=2)
    else:
        payload = as_text(meta, posts)

    if out_path == "-":
        sys.stdout.write(payload)
    else:
        if args.append and fmt == "text" and not old_posts and known_ids:
            # Text stubs carry no bodies, so append the new posts to the file
            # instead of rewriting it from records we don't have.
            body = as_text(meta, posts).split("-" * 72, 1)[1].lstrip("\n")
            with open(out_path, "a", encoding="utf-8") as f:
                f.write(body)
        else:
            with open(out_path, "w", encoding="utf-8") as f:
                f.write(payload)
        log(f"wrote {len(posts)} posts to {out_path} ({added} new)")


if __name__ == "__main__":
    main()
