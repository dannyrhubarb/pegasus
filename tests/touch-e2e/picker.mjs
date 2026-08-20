#!/usr/bin/env node
// Regression test for the replay bar's VS compare picker (see the "Compare
// ghost" section in CLAUDE.md).
//
// The picker's rows were first wired with onTap, the helper the corner
// buttons and the speed menu use. onTap fires on `touchstart` and
// preventDefault()s it, which is correct for a handful of fixed options that
// never scroll — and wrong for this list, which holds a whole score board
// (up to 50 entries) inside a scrolling panel. preventDefault on touchstart
// suppresses the scroll gesture outright, so the list could not be scrolled
// at ALL on a phone, and the first drag fired a pick on whatever row sat
// under the finger: a CDN fetch plus a ghost nobody asked for, with every
// entry below the fold unreachable. The fix is plain `click`, the same
// semantics the score board's own rows use — a click is not synthesized
// after a scroll gesture.
//
// Why a browser test: this is entirely a question of what Chrome does with a
// touch sequence when a listener calls preventDefault. It cannot be reasoned
// about from the source — this repo has already been burned twice by touch
// fixes that looked right (docs/touch-input.md) — and it reproduces exactly
// and deterministically under a synthesized touch drag.
//
// Two cases, matching run.mjs's shape:
//   control    a tap on a row          — must pick exactly once
//   regression a drag across the list  — must scroll and pick NOTHING
// If the control fails too, this harness is broken, not the game.
//
// Needs no wasm: the picker is pure DOM/CSS, so the page is driven directly.
//
// Usage: node picker.mjs
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");

const FILES = {
  "/index.html": [path.join(ROOT, "index.html"), "text/html"],
  "/mq_js_bundle.js": [path.join(ROOT, "mq_js_bundle.js"), "text/javascript"],
};

const server = http.createServer((req, res) => {
  const url = req.url.split("?")[0];
  const hit = FILES[url === "/" ? "/index.html" : url];
  if (!hit) {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
    return;
  }
  res.writeHead(200, { "content-type": hit[1] });
  fs.createReadStream(hit[0]).pipe(res);
});

// Fill the picker with a board big enough to overflow its panel and open it.
// pickCompare is stubbed so a pick is recorded instead of hitting the
// network — what's under test is which gestures reach it, not what it does.
const OPEN_PICKER = () => {
  document.getElementById("menu").style.display = "none";
  compareLevelFile = "expanse.level";
  compareActivePath = null;
  compareCandidates = Array.from({ length: 30 }, (_, i) => ({
    rank: i + 1, name: "PILOT" + i, score: 5000 - i * 50,
    ts: Date.now() - i * 86400e3, replayPath: "r" + i,
  }));
  window.__picks = [];
  window.pickCompare = (p) => window.__picks.push(p);
  renderGhostMenu();
  document.getElementById("replay-bar").classList.add("on");
  document.getElementById("rp-ghost").style.display = "";
  document.getElementById("rp-ghost-menu").classList.add("open");
};

async function newPage(browser, base) {
  const ctx = await browser.newContext({
    viewport: { width: 390, height: 780 }, hasTouch: true, isMobile: true,
  });
  const page = await ctx.newPage();
  await page.goto(base + "/index.html", { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => typeof renderGhostMenu === "function");
  await page.evaluate(OPEN_PICKER);
  return { ctx, page };
}

async function dragCase(browser, base) {
  const { ctx, page } = await newPage(browser, base);
  const list = page.locator("#rp-ghost-list");
  const box = await list.boundingBox();
  const cx = Math.round(box.x + box.width / 2);
  const cy = Math.round(box.y + box.height / 2);
  const cdp = await ctx.newCDPSession(page);

  const before = await list.evaluate((el) => el.scrollTop);
  await cdp.send("Input.dispatchTouchEvent",
    { type: "touchStart", touchPoints: [{ x: cx, y: cy, id: 1 }] });
  for (let i = 1; i <= 6; i++) {
    await cdp.send("Input.dispatchTouchEvent",
      { type: "touchMove", touchPoints: [{ x: cx, y: cy - i * 18, id: 1 }] });
    await page.waitForTimeout(16);
  }
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await page.waitForTimeout(400);

  const after = await list.evaluate((el) => el.scrollTop);
  const picks = await page.evaluate(() => window.__picks.slice());
  await ctx.close();
  return { scrolled: after > before, before, after, picks };
}

async function tapCase(browser, base) {
  const { ctx, page } = await newPage(browser, base);
  // Row 0 is "No ghost"; row 2 is the second real board entry.
  await page.locator("#rp-ghost-list .row").nth(2).tap();
  await page.waitForTimeout(400);
  const picks = await page.evaluate(() => window.__picks.slice());
  await ctx.close();
  return { picks };
}

await new Promise((res) => server.listen(0, "127.0.0.1", res));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch(
  process.env.PLAYWRIGHT_BROWSERS_PATH
    ? { executablePath: process.env.PW_CHROMIUM || undefined }
    : {},
);

let failed = false;
const report = (name, ok, detail) => {
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? " — " + detail : ""}`);
  if (!ok) failed = true;
};

const drag = await dragCase(browser, base);
report("regression: a scroll drag scrolls the list",
  drag.scrolled, `scrollTop ${drag.before} -> ${drag.after}`);
report("regression: a scroll drag picks nothing",
  drag.picks.length === 0, `picks ${JSON.stringify(drag.picks)}`);

const tap = await tapCase(browser, base);
report("control: a tap on a row picks exactly once",
  tap.picks.length === 1, `picks ${JSON.stringify(tap.picks)}`);

await browser.close();
server.close();
process.exit(failed ? 1 : 0);
