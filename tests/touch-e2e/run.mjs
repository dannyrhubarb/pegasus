#!/usr/bin/env node
// Regression test for the touch-claim bug (see docs/touch-input.md).
//
// The floating stick used to claim a touch only when macroquad reported
// `TouchPhase::Started`. macroquad keeps ONE entry per touch id and
// overwrites it wholesale, so a `touchstart` followed by a `touchmove`
// before the next frame collapses into a single `Moved` entry and the
// `Started` phase is never observed — the finger went dead for its whole
// press. An Android tester hit this constantly ("only every few touches
// goes through"); it depended on touch events arriving faster than frames.
//
// Why this test and not an emulator one: the bug is a RACE. On a device you
// cannot make the two events land in the same frame gap on demand, so an
// emulator test would pass or fail by luck. Dispatching both events
// synchronously in ONE JS task makes the collapse a certainty — the browser
// cannot run a frame between them — so this reproduces deterministically.
//
// Two cases run against the real wasm build:
//   control    touchstart alone           — must arm the run before AND after
//   regression touchstart + touchmove     — armed only after the fix
// The control is what tells a genuine regression apart from a broken
// harness: if BOTH fail, this file is wrong, not the game.
//
// Usage: cargo build --release --target wasm32-unknown-unknown && node run.mjs
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const WASM = path.join(ROOT, "target/wasm32-unknown-unknown/release/pegasus.wasm");

if (!fs.existsSync(WASM)) {
  console.error(`missing ${WASM}\nrun: cargo build --release --target wasm32-unknown-unknown`);
  process.exit(1);
}

// Deliberately minimal: NO levels/manifest.json, so the level picker is
// skipped and "Fly" drops straight into flight on the compiled-in demo
// level. Every other file the page asks for (config.json, version.json,
// whats-new.json, icons) is optional and designed to tolerate a 404.
const FILES = {
  "/index.html": [path.join(ROOT, "index.html"), "text/html"],
  "/mq_js_bundle.js": [path.join(ROOT, "mq_js_bundle.js"), "text/javascript"],
  "/pegasus.wasm": [WASM, "application/wasm"],
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

// Dispatch a touch down (and optionally a move) on the canvas. Both events
// go out in ONE task, which is the whole point — see the header.
function touchScript(withMove) {
  return (move) => {
    const c = document.getElementById("glcanvas");
    const r = c.getBoundingClientRect();
    const x = Math.round(r.left + r.width / 2);
    const y = Math.round(r.top + r.height / 2);
    const ev = (type, cx, cy) => {
      const t = new Touch({ identifier: 7, target: c, clientX: cx, clientY: cy });
      return new TouchEvent(type, {
        cancelable: true, bubbles: true,
        touches: [t], targetTouches: [t], changedTouches: [t],
      });
    };
    c.dispatchEvent(ev("touchstart", x, y));
    // No await, no rAF in between: the frame loop cannot run here, so
    // macroquad's entry for touch id 7 is overwritten Started -> Moved.
    if (move) c.dispatchEvent(ev("touchmove", x + 4, y + 4));
  };
}

const frames = (n) =>
  new Promise((res) => {
    let i = 0;
    const tick = () => (++i < n ? requestAnimationFrame(tick) : res());
    requestAnimationFrame(tick);
  });

async function runCase(browser, base, { withMove, name }) {
  const ctx = await browser.newContext({
    hasTouch: true,
    viewport: { width: 412, height: 915 },
    deviceScaleFactor: 2,
  });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  try {
    await page.goto(`${base}/index.html`);
    // The wasm is live once its exports are attached.
    await page.waitForFunction(
      () => typeof wasm_exports !== "undefined" && !!wasm_exports.run_start_seq,
      null, { timeout: 60_000 },
    );
    // No level list ⇒ Fly skips the picker and closes the menu into flight.
    await page.click("#btn-fly");
    await page.waitForFunction(
      () => !document.getElementById("menu").classList.contains("open"),
      null, { timeout: 10_000 },
    );
    await page.evaluate(frames, 5);

    // The run is armed-but-idle until the first non-neutral input, and a
    // bare stick touch is non-neutral — so run_start_seq bumping IS "the
    // game accepted the touch". An existing export; no test-only surface.
    const before = await page.evaluate(() => wasm_exports.run_start_seq());
    await page.evaluate(touchScript(withMove), withMove);
    await page.evaluate(frames, 12);
    const after = await page.evaluate(() => wasm_exports.run_start_seq());

    if (errors.length) throw new Error(`page errors: ${errors.join(" | ")}`);
    return { name, ok: after > before, before, after };
  } finally {
    await ctx.close();
  }
}

// CHROMIUM_PATH lets a machine that already has a Chromium (a preinstalled
// one whose build number doesn't match this playwright pin) skip the
// download. CI just runs `npx playwright install chromium` and leaves it
// unset.
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
let failed = false;
try {
  await new Promise((res) => server.listen(0, "127.0.0.1", res));
  const base = `http://127.0.0.1:${server.address().port}`;

  const cases = [
    { name: "control: touchstart alone arms the run", withMove: false },
    { name: "regression: touchstart+touchmove in one task arms the run", withMove: true },
  ];
  const results = [];
  for (const c of cases) results.push(await runCase(browser, base, c));

  for (const r of results) {
    console.log(`${r.ok ? "PASS" : "FAIL"}  ${r.name}  (run_start_seq ${r.before} -> ${r.after})`);
    if (!r.ok) failed = true;
  }
  if (!results[0].ok) {
    console.error(
      "\nThe CONTROL case failed, so the harness could not drive the game at all " +
      "— fix this file before reading anything into the regression case.",
    );
  } else if (failed) {
    console.error(
      "\nThe game ignored a touch whose Started phase was collapsed by a same-task " +
      "touchmove. This is the Android 'only every few touches goes through' bug: " +
      "the stick must claim a touch by IDENTITY (an id absent last frame), never by " +
      "TouchPhase::Started. See docs/touch-input.md.",
    );
  }
} finally {
  await browser.close();
  server.close();
}
process.exit(failed ? 1 : 0);
