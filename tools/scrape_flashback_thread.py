#!/usr/bin/env python3
"""Scrape every post from every page of a Flashback forum thread.

Stdlib-only (no pip installs). Fetches all pages of a thread like
https://www.flashback.org/t3739595, extracts each post (number, id, date,
author, message text incl. quoted posts) and writes JSON or plain text.

Usage:
    python3 scrape_flashback_thread.py https://www.flashback.org/t3739595
    python3 scrape_flashback_thread.py t3739595 -f text -o thread.txt
    python3 scrape_flashback_thread.py 3739595 --pages 1-5 -o - | jq .

By default the output lands in t<thread-id>.json / .txt next to where you
run it; pass `-o -` for stdout. A polite delay (default 1 s) is kept
between page fetches.
"""

import argparse
import html
import json
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

BASE = "https://www.flashback.org"
USER_AGENT = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/126.0 Safari/537.36"
)


def log(msg):
    print(msg, file=sys.stderr, flush=True)


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


def parse_page(page_html, page_no):
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

        posts.append({
            "page": page_no,
            "number": int(num.group(1)) if num else None,
            "post_id": pid,
            "date": html.unescape(f"{date.group(1)}, {date.group(2)}") if date else None,
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
        out.append(f"#{p['number']} | {who} (u{p['user_id']}) | {p['date']} | {p['url']}")
        out.append("")
        out.append(p["message"])
        out.append(sep)
    return "\n".join(out) + "\n"


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
    ap.add_argument("--delay", type=float, default=1.0,
                    help="seconds to wait between page fetches (default 1.0)")
    args = ap.parse_args()

    tid = thread_id_from(args.thread)
    fmt = args.format
    if fmt is None and args.output and args.output.endswith(".txt"):
        fmt = "text"
    fmt = fmt or "json"
    out_path = args.output or f"t{tid}.{'txt' if fmt == 'text' else 'json'}"

    log(f"fetching {page_url(tid, 1)} ...")
    first = fetch(page_url(tid, 1))

    m = re.search(r"<title>(.*?)</title>", first, re.S)
    title = html.unescape(m.group(1)).replace(" - Flashback Forum", "").strip() if m else None
    m = re.search(r"Sidan\s+\d+\s+av\s+(\d+)", first)
    total = int(m.group(1)) if m else 1

    lo, hi = 1, total
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

    posts = []
    for page in range(lo, hi + 1):
        page_html = first if page == 1 else None
        if page_html is None:
            time.sleep(args.delay)
            page_html = fetch(page_url(tid, page))
        got = parse_page(page_html, page)
        posts.extend(got)
        log(f"  page {page}/{hi}: {len(got)} posts (total {len(posts)})")

    meta = {
        "thread_id": tid,
        "url": f"{BASE}/t{tid}",
        "title": title,
        "pages": total,
        "scraped_pages": [lo, hi],
        "scraped_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "post_count": len(posts),
    }

    if fmt == "json":
        payload = json.dumps({**meta, "posts": posts}, ensure_ascii=False, indent=2)
    else:
        payload = as_text(meta, posts)

    if out_path == "-":
        sys.stdout.write(payload)
    else:
        with open(out_path, "w", encoding="utf-8") as f:
            f.write(payload)
        log(f"wrote {len(posts)} posts to {out_path}")


if __name__ == "__main__":
    main()
