#!/usr/bin/env python3
"""Generate third-party-licenses.html from the wasm build's dependency tree.

Re-run whenever Cargo.lock changes (needs a prior `cargo build` so the
registry sources are on disk):

    python3 tools/gen-third-party-licenses.py

The page lists every crate resolved for the wasm32-unknown-unknown target
(the shipped binary) plus the vendored miniquad JS loader, groups them by
elected license, extracts each crate's real copyright notices from its
packaged license files, and appends one copy of each license's full text.
"""

import html
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "third-party-licenses.html"

# When a crate offers a license choice (SPDX OR), we elect ONE license to
# comply with, in this preference order (shortest/simplest obligations first).
ELECTION_ORDER = ["MIT", "Zlib", "0BSD", "Unlicense", "BSD-3-Clause", "Apache-2.0", "Unicode-3.0"]

FULL_TEXTS = {
    "MIT": """\
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.""",
    "Zlib": """\
zlib License

This software is provided 'as-is', without any express or implied warranty.
In no event will the authors be held liable for any damages arising from the
use of this software.

Permission is granted to anyone to use this software for any purpose,
including commercial applications, and to alter it and redistribute it
freely, subject to the following restrictions:

1. The origin of this software must not be misrepresented; you must not
   claim that you wrote the original software. If you use this software in a
   product, an acknowledgment in the product documentation would be
   appreciated but is not required.

2. Altered source versions must be plainly marked as such, and must not be
   misrepresented as being the original software.

3. This notice may not be removed or altered from any source distribution.""",
    "Apache-2.0": None,  # filled in from a crate's packaged LICENSE-APACHE below
    "Unicode-3.0": """\
UNICODE LICENSE V3

Permission is hereby granted, free of charge, to any person obtaining a copy
of data files and any associated documentation (the "Data Files") or software
and any associated documentation (the "Software") to deal in the Data Files
or Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, and/or sell copies of the Data
Files or Software, and to permit persons to whom the Data Files or Software
are furnished to do so, provided that either (a) this copyright and
permission notice appear with all copies of the Data Files or Software, or
(b) this copyright and permission notice appear in associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS
INCLUDED IN THIS NOTICE BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR
CONSEQUENTIAL DAMAGES, OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE,
DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
OF THE DATA FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall not
be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written authorization
of the copyright holder.""",
}


def wasm_dep_set():
    """(name, version) pairs resolved for the wasm target, excluding the
    first-party workspace crates (pegasus + path members like pegasus-sim,
    which cargo tree marks with an absolute path in parens)."""
    out = subprocess.run(
        ["cargo", "tree", "--target", "wasm32-unknown-unknown", "-e", "normal",
         "--prefix", "none"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout
    deps = set()
    for line in out.splitlines():
        m = re.match(r"^(\S+) v(\S+)", line.strip())
        if m and "(/" not in line:
            deps.add((m.group(1), m.group(2)))
    return deps


def metadata_index():
    """{(name, version): package} from cargo metadata."""
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout
    meta = json.loads(out)
    return {(p["name"], p["version"]): p for p in meta["packages"]}


def elect(spdx):
    """Pick the licenses we comply with from an SPDX expression.

    OR = a choice (pick one per ELECTION_ORDER); AND = all terms apply.
    """
    spdx = spdx.replace("/", " OR ")  # legacy "MIT/Apache-2.0" form
    elected = []
    for conj in re.split(r"\bAND\b", spdx):
        options = [o.strip().strip("()") for o in re.split(r"\bOR\b", conj)]
        for pref in ELECTION_ORDER:
            if pref in options:
                elected.append(pref)
                break
        else:
            elected.append(options[0])  # unknown license: keep it visible
    return elected


def copyright_lines(pkg_dir, elected):
    """Extract distinct copyright lines from the crate's license files."""
    # standard "Copyright (c) 2019 ..." plus the bare "@ 2019-2020 Name" form
    # miniquad/macroquad use in their LICENSE-MIT
    pat = re.compile(r"^\s*(copyright\s+(\(c\)|©|\d).*|[@©]\s*\d{4}.*)$", re.IGNORECASE)
    names = []
    for lic in elected:
        # prefer the license file matching the elected license
        names += [f"LICENSE-{lic.upper().split('-')[0]}*", f"LICENSES/{lic}*"]
    names += ["LICENSE*", "COPYING*", "COPYRIGHT*"]
    lines, seen = [], set()
    for pattern in names:
        for f in sorted(pkg_dir.glob(pattern)):
            if not f.is_file():
                continue
            try:
                text = f.read_text(errors="replace")
            except OSError:
                continue
            for raw in text.splitlines():
                m = pat.match(raw)
                if m:
                    line = re.sub(r"\s+", " ", m.group(1)).strip()
                    if line.lower() not in seen and len(line) < 200:
                        seen.add(line.lower())
                        lines.append(line)
        if lines:
            break  # the matched license file is authoritative; stop widening
    return lines


def apache_text(index, deps):
    """Grab the canonical Apache-2.0 text from a crate that packages it."""
    for name, ver in sorted(deps):
        pkg = index.get((name, ver))
        if not pkg:
            continue
        pkg_dir = Path(pkg["manifest_path"]).parent
        for f in sorted(pkg_dir.glob("LICENSE-APACHE*")):
            text = f.read_text(errors="replace")
            if "Apache License" in text and "Version 2.0" in text:
                # strip any appendix copyright fill-in specific to that crate
                return text.strip()
    sys.exit("no packaged Apache-2.0 text found — run `cargo build` first")


def build_page(entries, texts):
    e = html.escape
    css = """
  body { margin: 0; padding: 24px 16px 64px; background: #060913; color: #cdd6e4;
         font: 14px/1.55 "Courier New", monospace; }
  main { max-width: 860px; margin: 0 auto; }
  h1 { color: #7df9ff; letter-spacing: 3px; text-shadow: 0 0 12px rgba(125,249,255,.55);
       font-size: 26px; }
  h2 { color: #ff5ff1; letter-spacing: 2px; margin-top: 40px; font-size: 18px;
       text-shadow: 0 0 10px rgba(255,95,241,.45); }
  a { color: #7df9ff; }
  .crate { margin: 14px 0; padding: 10px 14px; border: 1px solid #1c2b4a;
           border-radius: 8px; background: rgba(20,30,55,.45); }
  .crate b { color: #ffd36b; }
  .lic { color: #7dffa8; font-size: 12px; }
  .cc { color: #8b98ad; font-size: 12px; margin: 2px 0 0 12px; }
  pre { white-space: pre-wrap; border: 1px solid #1c2b4a; border-radius: 8px;
        padding: 14px; background: rgba(10,16,32,.7); font-size: 12px; color: #aab6c8; }
  .note { color: #8b98ad; }
"""
    parts = [f"<meta charset='utf-8'><meta name='viewport' content='width=device-width, initial-scale=1'>",
             f"<title>Pegasus — third-party licenses</title><style>{css}</style><main>",
             "<h1>THIRD-PARTY LICENSES</h1>",
             "<p class='note'>Pegasus itself is free software, licensed under the "
             "<a href='https://www.gnu.org/licenses/gpl-3.0.html'>GNU GPL-3.0-or-later</a>; "
             "the complete source is at "
             "<a href='https://github.com/dannyrhubarb/pegasus'>github.com/dannyrhubarb/pegasus</a> "
             "(license text: <a href='LICENSE'>LICENSE</a>, served with this site). "
             "The game is built from the open-source components below; where a component offers a "
             "choice of licenses, the one shown is the license Pegasus elects. "
             "Full license texts follow the component list.</p>",
             "<h2>JAVASCRIPT LOADER</h2>",
             "<div class='crate'><b>miniquad JS bundle</b> (mq_js_bundle.js, vendored from "
             "<a href='https://github.com/not-fl3/miniquad'>not-fl3/miniquad</a>) "
             "<span class='lic'>MIT</span>"
             "<div class='cc'>© 2019-2020 Fedor Logachev &lt;not.fl3@gmail.com&gt;</div></div>",
             "<h2>RUST CRATES (compiled into pegasus.wasm)</h2>"]
    for name, ver, spdx, elected, ccs in entries:
        parts.append(f"<div class='crate'><b>{e(name)}</b> {e(ver)} "
                     f"<span class='lic'>{e(' + '.join(elected))}</span> "
                     f"<span class='note'>(declared: {e(spdx)})</span>")
        for c in ccs:
            parts.append(f"<div class='cc'>{e(c)}</div>")
        if not ccs:
            parts.append("<div class='cc'>(no copyright line in packaged license files)</div>")
        parts.append("</div>")
    parts.append("<h2>LICENSE TEXTS</h2>")
    parts.append("<p class='note'>One copy of each elected license; the copyright "
                 "notices above apply per component.</p>")
    for lic in sorted(texts):
        parts.append(f"<h2 id='{e(lic)}'>{e(lic)}</h2><pre>{e(texts[lic])}</pre>")
    parts.append("</main>")
    return "\n".join(parts) + "\n"


def main():
    deps = wasm_dep_set()
    index = metadata_index()
    entries, needed = [], set()
    for name, ver in sorted(deps):
        pkg = index.get((name, ver))
        if not pkg:
            sys.exit(f"{name} {ver} in cargo tree but not in metadata")
        spdx = pkg["license"] or "?"
        elected = elect(spdx)
        needed.update(elected)
        ccs = copyright_lines(Path(pkg["manifest_path"]).parent, elected)
        entries.append((name, ver, spdx, elected, ccs))
    texts = {}
    for lic in needed:
        if lic == "Apache-2.0":
            texts[lic] = apache_text(index, deps)
        elif lic in FULL_TEXTS and FULL_TEXTS[lic]:
            texts[lic] = FULL_TEXTS[lic]
        else:
            texts[lic] = f"(text not bundled — see https://spdx.org/licenses/{lic}.html)"
    OUT.write_text(build_page(entries, texts))
    print(f"wrote {OUT} ({OUT.stat().st_size // 1024} KB, {len(entries)} crates, "
          f"licenses: {', '.join(sorted(needed))})")


if __name__ == "__main__":
    main()
