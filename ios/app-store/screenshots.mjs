#!/usr/bin/env node
// App Store screenshot generator — drives the REAL built site (site/, from
// tools/build-site.sh) in headless Chromium at Apple's required pixel sizes
// and captures the screens a store visitor should see. Raw captures land in
// screenshots/raw/<device>/; compose.mjs turns them into the captioned
// marketing set. See README.md in this folder.
//
//   GIT_REV=$(git rev-parse --short HEAD) tools/build-site.sh
//   curl -o site/config.json https://pegasusmoonlander.com/config.json   # optional: live boards + ghost
//   cd ios/app-store && npm ci && npm run shots
//
// Device sets (App Store Connect, 2026): the 6.9" iPhone and the 13" iPad
// are the two REQUIRED sets — every smaller size scales down from them.
//   iPhone 6.9"  1320×2868 (440×956 CSS px @3x)   iPhone 16 Pro Max class
//   iPad 13"     2064×2752 (1032×1376 CSS px @2x)  iPad Pro 13" class
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const SITE = path.join(ROOT, "site");
const OUT = path.join(ROOT, "ios/app-store/screenshots/raw");
if (!fs.existsSync(path.join(SITE, "pegasus.wasm"))) {
  console.error("missing site/pegasus.wasm — run: GIT_REV=$(git rev-parse --short HEAD) tools/build-site.sh");
  process.exit(1);
}
const online = fs.existsSync(path.join(SITE, "config.json"));
console.log(online ? "config.json present: boards, ghost and replays are LIVE data"
                   : "no site/config.json: offline set (no boards/ghost/replay shots)");

const MIME = {
  ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm",
  ".json": "application/json", ".png": "image/png", ".woff2": "font/woff2",
  ".level": "text/plain", ".svg": "image/svg+xml", ".txt": "text/plain",
};
// RELAY=1: route the backend through this server via curl instead of letting
// Chromium talk to the internet itself — for sandboxes whose egress proxy
// the browser does not trust (curl does). config.json is rewritten on the
// fly to point at /__relay/<host>/...; the file on disk is untouched.
const RELAY = !!process.env.RELAY;
const cfg = online ? JSON.parse(fs.readFileSync(path.join(SITE, "config.json"), "utf8")) : null;
const server = http.createServer((req, res) => {
  let url = decodeURIComponent(req.url.split("?")[0]);
  if (url === "/") url = "/index.html";
  if (RELAY && cfg && url === "/config.json") {
    const relay = (u) => `http://${req.headers.host}/__relay/${new URL(u).host}`;
    res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" });
    res.end(JSON.stringify({ ...cfg, apiBaseUrl: relay(cfg.apiBaseUrl),
                             replayBaseUrl: relay(cfg.replayBaseUrl) }));
    return;
  }
  if (RELAY && url.startsWith("/__relay/")) {
    const [, , host, ...rest] = url.split("/");
    const q = req.url.includes("?") ? req.url.slice(req.url.indexOf("?")) : "";
    const upstream = `https://${host}/${rest.join("/")}${q}`;
    const c = spawn("curl", ["-sS", "-L", "--max-time", "30", "-w", "\\n%{http_code}", upstream]);
    const chunks = [];
    c.stdout.on("data", (d) => chunks.push(d));
    c.on("close", () => {
      const buf = Buffer.concat(chunks);
      const nl = buf.lastIndexOf(0x0a);
      const status = parseInt(buf.subarray(nl + 1).toString(), 10) || 502;
      res.writeHead(status, { "access-control-allow-origin": "*", "cache-control": "no-store" });
      res.end(buf.subarray(0, nl));
    });
    return;
  }
  const file = path.join(SITE, url);
  if (!file.startsWith(SITE) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
    res.writeHead(404, { "content-type": "text/plain" }); res.end("not found"); return;
  }
  res.writeHead(200, { "content-type": MIME[path.extname(file)] || "application/octet-stream",
                       "cache-control": "no-store" });
  fs.createReadStream(file).pipe(res);
});

// Portrait is the primary orientation (the store shows portrait sets first);
// landscape variants of the flight shots come from a second pass.
const DEVICES = [
  { name: "iphone-6.9", w: 440,  h: 956,  dpr: 3, mobile: true },
  { name: "ipad-13",    w: 1032, h: 1376, dpr: 2, mobile: false },
];
const only = process.env.DEVICE; // e.g. DEVICE=iphone-6.9 for a quick run

// In-page multi-touch helper: keeps the active fingers so every TouchEvent
// carries the full `touches` list (macroquad reads touches(), not clicks).
const TOUCH_HELPER = `
  window.__fingers = new Map();
  window.__touch = (type, id, x, y) => {
    const c = document.getElementById("glcanvas");
    if (type === "end") {
      const t = window.__fingers.get(id); window.__fingers.delete(id);
      const all = [...window.__fingers.values()];
      c.dispatchEvent(new TouchEvent("touchend", { cancelable: true, bubbles: true,
        touches: all, targetTouches: all, changedTouches: t ? [t] : [] }));
      return;
    }
    const t = new Touch({ identifier: id, target: c, clientX: x, clientY: y });
    window.__fingers.set(id, t);
    const all = [...window.__fingers.values()];
    c.dispatchEvent(new TouchEvent(type === "start" ? "touchstart" : "touchmove",
      { cancelable: true, bubbles: true, touches: all, targetTouches: all, changedTouches: [t] }));
  };
`;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function waitWasm(page) {
  await page.waitForFunction(() => typeof wasm_exports !== "undefined" && !!wasm_exports.ui_state,
    null, { timeout: 90_000 });
}
async function waitScreen(page, id) {
  await page.waitForFunction((id) => document.getElementById(id).classList.contains("on"), id,
    { timeout: 15_000 });
}
async function waitMenuClosed(page) {
  await page.waitForFunction(() => !document.getElementById("menu").classList.contains("open"),
    null, { timeout: 15_000 });
}
const touch = (page, type, id, x, y) => page.evaluate(([t, i, x, y]) => window.__touch(t, i, x, y), [type, id, x, y]);
async function pickLevel(page, name) {
  await page.click("#btn-fly");
  await waitScreen(page, "scr-levels");
  await page.locator("#level-list .row", { hasText: name }).first().click();
  await waitMenuClosed(page);
}
async function backHome(page) {
  // From flight: ✕ → pause screen → Exit to menu (ends the run, never submits).
  await page.locator("#pause-btn").dispatchEvent("click"); // corner buttons: onTap listens for click
  await waitScreen(page, "scr-pause");
  await page.click("#btn-exit");
  await waitScreen(page, "scr-home");
}

// One short burn with the split controls (throttle button on the left half,
// steering stick on the right): the ship lifts off the spawn pad with the
// exhaust plume lit and both widgets under the fingers.
async function flyBurst(page, d, { burnMs = 550, steer = [26, -34] } = {}) {
  const lx = d.w * 0.22, rx = d.w * 0.78, y = d.h * 0.72;
  await touch(page, "start", 1, lx, y);              // throttle
  await touch(page, "start", 2, rx, y);              // stick centre
  await sleep(120);
  await touch(page, "move", 2, rx + steer[0], y + steer[1]); // nose up-right
  await sleep(burnMs); // screenshot follows with the throttle still held
}
async function release(page) { await touch(page, "end", 1); await touch(page, "end", 2); }

async function shoot(page, dir, name) {
  const file = path.join(dir, `${name}.png`);
  await page.screenshot({ path: file, type: "png" });
  console.log("  wrote", path.relative(ROOT, file));
}

async function runDevice(browser, base, d, landscape) {
  const w = landscape ? d.h : d.w, h = landscape ? d.w : d.h;
  const dir = path.join(OUT, d.name + (landscape ? "-landscape" : ""));
  fs.mkdirSync(dir, { recursive: true });
  console.log(`${d.name}${landscape ? " landscape" : ""}: ${w * d.dpr}×${h * d.dpr}`);
  const ctx = await browser.newContext({
    hasTouch: true, isMobile: d.mobile, viewport: { width: w, height: h }, deviceScaleFactor: d.dpr,
    reducedMotion: "no-preference",
  });
  await ctx.addInitScript(TOUCH_HELPER);
  await ctx.addInitScript(() => {
    try {
      localStorage.setItem("pegasus_level", "expanse.level");
      localStorage.setItem("pegasus_sound", "0");
    } catch (e) {}
  });
  const page = await ctx.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  try {
    await page.goto(`${base}/index.html`);
    await waitWasm(page);
    await sleep(1500);
    if (!landscape) await shoot(page, dir, "01-home");

    // Level picker (fly mode) with the live records filled in.
    await page.click("#btn-fly");
    await waitScreen(page, "scr-levels");
    await sleep(online ? 3000 : 500);
    if (!landscape) await shoot(page, dir, "02-levels");
    await page.locator("#level-list .row", { hasText: "The Expanse" }).first().click();
    await waitMenuClosed(page);
    await sleep(online ? 2500 : 800); // ghost blob fetch
    await flyBurst(page, { w, h });
    await shoot(page, dir, "03-flight-expanse");
    await release(page);

    // The Hollows: the hand-drawn chamber level.
    await backHome(page);
    await pickLevel(page, "The Hollows");
    await sleep(online ? 2500 : 800);
    await flyBurst(page, { w, h }, { burnMs: 450, steer: [-30, -30] });
    await shoot(page, dir, "04-flight-hollows");
    await release(page);
    await backHome(page);

    if (online) {
      // Global board for The Expanse, then a stored replay with the transport bar.
      await page.click("#btn-scores");
      await waitScreen(page, "scr-levels");
      await page.locator("#level-list .row", { hasText: "The Expanse" }).first().click();
      await waitScreen(page, "scr-scores");
      await page.waitForFunction(() => document.querySelectorAll("#scores-list li").length >= 5,
        null, { timeout: 20_000 });
      await sleep(1200);
      if (!landscape) await shoot(page, dir, "05-scores");
      await page.locator("#scores-list .watch").first().click();
      await page.waitForFunction(() => wasm_exports.ui_state() === 3, null, { timeout: 30_000 });
      await sleep(6000);
      // The transport GUI auto-hides after 2.5 s; a REAL tap (pointerdown, not
      // a synthetic TouchEvent) on the canvas brings it back.
      await page.touchscreen.tap(w / 2, h / 2);
      await sleep(400);
      await shoot(page, dir, "06-replay");
      await page.locator("#exit-replay-btn").dispatchEvent("click");
      await waitScreen(page, "scr-scores");
      await page.click("#btn-scores-back");
      await waitScreen(page, "scr-levels");
      await page.click("#scr-levels .mbtn.back");
      await waitScreen(page, "scr-home");
    }

    if (!landscape) {
      await page.click('[data-goto="scr-settings"]');
      await waitScreen(page, "scr-settings");
      await sleep(300);
      await shoot(page, dir, "07-settings");
      await page.click("#scr-settings .mbtn.back");
      await waitScreen(page, "scr-home");
      await page.click('[data-goto="scr-about"]');
      await waitScreen(page, "scr-about");
      await page.click("#btn-manual");
      await waitScreen(page, "scr-manual");
      await sleep(300);
      await shoot(page, dir, "08-manual");
    }
    if (errors.length) console.warn("  page errors:", errors.join(" | "));
  } finally {
    await ctx.close();
  }
}

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
try {
  await new Promise((res) => server.listen(0, "127.0.0.1", res));
  const base = `http://127.0.0.1:${server.address().port}`;
  for (const d of DEVICES) {
    if (only && d.name !== only) continue;
    await runDevice(browser, base, d, false);
    if (!process.env.NO_LANDSCAPE) await runDevice(browser, base, d, true);
  }
} finally {
  await browser.close();
  server.close();
}
