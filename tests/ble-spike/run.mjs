#!/usr/bin/env node
// Headless check for the BLE nearby-link SPIKE's page half (index.html's
// pegBle module + the scr-ble screen) — see docs/multiplayer-ble.md.
//
// There is no radio here: two pages (host + guest) each get a FAKE
// `window.PegasusBle` bridge whose commands land in this process, which
// plays the radio — advertise/scan/connect state, and PDU delivery to the
// other page as `pegBle._on({ev:"data"})` events. What it proves is
// exactly the part native can't: the page's framing survives an arbitrary
// PDU chunking (the fake enforces the ATT-default 20-byte payload and
// FAILS on any oversized PDU), the reassembly is in order, the screen's
// flow works end to end (host → find → connect → hello → ping burst →
// 10 s stream → report), and a 1000-byte message crosses in 51 PDUs.
//
// The real bridges are exercised on devices only (there is no BLE in CI);
// this keeps the shared codec honest between those sessions.
//
// Usage: cd tests/ble-spike && npm ci && node run.mjs
//        (CHROMIUM_PATH=/path/to/chrome skips the browser download)
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
// No wasm, no manifest: the menu is markup and the spike screen is pure JS,
// so the page needs neither (every other file it probes for tolerates a 404).
const server = http.createServer((req, res) => {
  const url = req.url.split("?")[0];
  const hit = FILES[url === "/" ? "/index.html" : url];
  if (!hit) { res.writeHead(404, { "content-type": "text/plain" }); res.end("not found"); return; }
  res.writeHead(200, { "content-type": hit[1] });
  fs.createReadStream(hit[0]).pipe(res);
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const BASE = `http://127.0.0.1:${server.address().port}`;

const MTU = 23; // the ATT default — the harshest chunking
let failures = 0;
function check(cond, msg) {
  console.log(`${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
}

// CHROMIUM_PATH lets a machine with a preinstalled Chromium (this repo's
// touch-e2e uses the same knob) run without `npx playwright install`.
const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
const sides = {};
let pduCount = 0;

// The fake radio: one state per side, events delivered in order per page.
function makeSide(name, callsign) {
  const side = { name, callsign, page: null, state: "idle", queue: Promise.resolve() };
  side.emit = evt => {
    side.queue = side.queue.then(() =>
      side.page.evaluate(e => window.pegBle._on(e), evt)).catch(e => console.error("emit failed", e));
    return side.queue;
  };
  side.cmd = json => {
    const c = JSON.parse(json);
    const other = sides[name === "host" ? "guest" : "host"];
    switch (c.cmd) {
      case "host":
        side.state = "advertising"; side.adv = c.name;
        side.emit({ ev: "state", state: "advertising", role: "host" });
        if (other.state === "scanning") other.emit({ ev: "peer", id: "AA:BB:CC:DD:EE:FF", name: c.name, rssi: -48 });
        break;
      case "scan":
        side.state = "scanning";
        side.emit({ ev: "state", state: "scanning", role: "guest" });
        if (other.state === "advertising") side.emit({ ev: "peer", id: "AA:BB:CC:DD:EE:FF", name: other.adv, rssi: -48 });
        break;
      case "connect":
        check(c.id === "AA:BB:CC:DD:EE:FF", "guest connects to the discovered id");
        side.emit({ ev: "state", state: "connecting" });
        side.state = other.state = "connected";
        side.emit({ ev: "state", state: "connected", role: "guest", mtu: MTU });
        other.emit({ ev: "state", state: "connected", role: "host", mtu: MTU });
        break;
      case "send": {
        const bytes = Buffer.from(c.b64, "base64");
        pduCount++;
        if (bytes.length > MTU - 3) check(false, `PDU of ${bytes.length} B exceeds mtu-3 (${MTU - 3})`);
        if (side.state === "connected") other.emit({ ev: "data", b64: c.b64 });
        break;
      }
      case "stop":
        if (side.state === "connected") { other.state = "disconnected"; other.emit({ ev: "state", state: "disconnected", reason: "peer left" }); }
        side.state = "idle";
        side.emit({ ev: "state", state: "idle", reason: "stopped" });
        break;
      default:
        check(false, `unknown command ${c.cmd}`);
    }
  };
  return side;
}

async function openSide(side) {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  const page = await ctx.newPage();
  side.page = page;
  page.on("pageerror", e => check(false, `${side.name} page error: ${e.message}`));
  await page.exposeFunction("__bleCmd", json => side.cmd(json));
  await page.addInitScript(cs => {
    window.PegasusBle = { cmd: j => window.__bleCmd(j) };
    localStorage.setItem("pegasus_debug_hud", "1");
    localStorage.setItem("pegasus_player_name", cs);
  }, side.callsign);
  await page.goto(`${BASE}/index.html`);
  await page.click('#scr-home [data-goto="scr-settings"]');
  await page.waitForFunction(() => getComputedStyle(document.getElementById("btn-ble")).display !== "none");
  await page.click("#btn-ble");
  await page.waitForSelector("#scr-ble.on");
  return page;
}

const logHas = (page, text) => page.waitForFunction(t => document.getElementById("ble-log").textContent.includes(t), text, { timeout: 20000 });
const status = page => page.$eval("#ble-status", el => el.textContent);

try {
  sides.host = makeSide("host", "HOSTPILOT");
  sides.guest = makeSide("guest", "GUESTPILOT");
  const A = await openSide(sides.host);
  const B = await openSide(sides.guest);
  check(await A.evaluate(() => window.pegBle.available()), "bridge detected");
  check(await A.evaluate(() => history.state && history.state.s === "scr-ble" && history.state.d === 2),
        "scr-ble is a depth-2 history entry (Settings → BLE)");

  // Host → find → connect.
  await A.click("#ble-host");
  await logHas(A, "state advertising");
  await B.click("#ble-scan");
  await B.waitForSelector("#ble-peers li.row");
  check((await B.$eval("#ble-peers li.row .lname", el => el.textContent)) === "HOSTPILOT", "guest lists the host by callsign");
  await B.click("#ble-peers li.row");
  await logHas(A, "hello from GUESTPILOT");
  await logHas(B, "hello from HOSTPILOT");
  check((await status(A)).includes("connected · host · mtu 23 (20 B/pdu)"), `host status: ${await status(A)}`);
  check((await status(B)).includes("peer HOSTPILOT"), `guest status names the peer: ${await status(B)}`);

  // Ping burst (guest → host → guest).
  await B.click("#ble-ping");
  await logHas(B, "ping: 20/20 replies");

  // 10 s stream at the game's batch cadence, host → guest, report back.
  await A.click("#ble-stream");
  await logHas(B, "stream: got 150/150");
  await logHas(A, "report: peer got 150/150");

  // A message far larger than one PDU: 1000 bytes → 51 PDUs of ≤ 20 B,
  // reassembled in order on the other side.
  await B.evaluate(() => { window.__big = null; window.pegBle.on("message", m => { if (m[0] === 0x7f) window.__big = Array.from(m); }); });
  const before = pduCount;
  await A.evaluate(() => { const b = new Uint8Array(1000); b[0] = 0x7f; for (let i = 1; i < b.length; i++) b[i] = i & 255; return window.pegBle.send(b); });
  await B.waitForFunction(() => window.__big !== null);
  const big = await B.evaluate(() => window.__big);
  check(big.length === 1000 && big.every((v, i) => v === (i === 0 ? 0x7f : i & 255)), "1000-byte message reassembles byte-exact");
  check(pduCount - before === 51, `1002 framed bytes crossed in ${pduCount - before} PDUs (expected 51)`);

  // Stop on one side → the other sees the disconnect.
  await B.click("#ble-stop");
  await logHas(A, "state disconnected");
  await logHas(B, "state idle");
  check((await status(A)).startsWith("disconnected"), `host status after peer stop: ${await status(A)}`);
} catch (e) {
  check(false, `harness: ${e.message}`);
}

await browser.close();
server.close();
console.log(failures ? `\n${failures} check(s) failed` : "\nall checks passed");
process.exit(failures ? 1 : 0);
