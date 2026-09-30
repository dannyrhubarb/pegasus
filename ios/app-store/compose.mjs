#!/usr/bin/env node
// Turns the raw captures (screenshots.mjs → screenshots/raw/) into the
// captioned App Store sets in screenshots/final/: same pixel size as the
// capture, a neon headline + one-line sub in the game's own JetBrains Mono,
// the capture below in a glowing rounded frame. Pure HTML/CSS rendered by
// headless Chromium at 1:1 — no image libraries.
//
//   cd ios/app-store && npm run compose        (after npm run shots)
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "../..");
const RAW = path.join(HERE, "screenshots/raw");
const FINAL = path.join(HERE, "screenshots/final");
const FONT = path.join(ROOT, "fonts/jetbrains-mono.woff2");

// Captions per raw capture. Order here = upload order (the first three show
// in App Store search results, so the game itself leads).
const CAPTIONS = [
  ["03-flight-expanse", "THREAD THE ROCK",        "Boulders, fuel, and a landing to stick."],
  ["04-flight-hollows", "FIVE PADS, ONE CLOCK",   "Hand-drawn chambers. Visit them all."],
  ["01-home",           "ONE STICK. REAL PHYSICS.", "Hold to burn, point to steer."],
  ["02-levels",         "TEN LEVELS",             "Distance, sprints, dashes, time trials."],
  ["05-scores",         "GLOBAL HIGH SCORES",     "Every entry is a verified replay."],
  ["06-replay",         "WATCH ANY RUN",          "Pause, scrub, slow-mo, frame by frame."],
  ["07-settings",       "YOUR CONTROLS",          "One-handed, split, or left-handed."],
];

const png = (p) => "data:image/png;base64," + fs.readFileSync(p).toString("base64");
const font = "data:font/woff2;base64," + fs.readFileSync(FONT).toString("base64");

function html({ img, w, h, landscape, head, sub }) {
  // Sizes scale with the short edge so phone and tablet canvases read alike.
  const u = Math.min(w, h) / 1000;
  const headFs = Math.round((landscape ? 62 : 74) * u);
  const subFs = Math.round((landscape ? 30 : 31) * u);
  const radius = Math.round(46 * u);
  // The capture keeps its whole frame (nothing bled off an edge — the
  // replay bar and the parked widgets sit at the bottom) and its own aspect.
  let shot;
  if (landscape) {
    const sw = Math.round(w * 0.62), sh = Math.round(sw * h / w);
    shot = { left: Math.round(w * 0.36), top: Math.round((h - sh) / 2), width: sw, height: sh };
  } else {
    const top = Math.round(96 * u + headFs * 2.5 + subFs * 2.6);
    const avail = h - top - Math.round(48 * u);
    const sw = Math.min(Math.round(w * 0.86), Math.round(avail * w / h)), sh = Math.round(sw * h / w);
    shot = { left: Math.round((w - sw) / 2), top, width: sw, height: sh };
  }
  const layout = landscape
    ? `#cap{position:absolute;left:${Math.round(70 * u)}px;top:0;bottom:0;width:${Math.round(w * 0.30)}px;
         display:flex;flex-direction:column;justify-content:center;text-align:left}`
    : `#cap{position:absolute;left:${Math.round(60 * u)}px;right:${Math.round(60 * u)}px;top:${Math.round(96 * u)}px;
         text-align:center}`;
  const shotCss = `#shot{position:absolute;left:${shot.left}px;top:${shot.top}px;width:${shot.width}px;height:${shot.height}px}`;
  return `<!doctype html><html><head><meta charset="utf-8"><style>
    @font-face{font-family:"JBM";src:url(${font}) format("woff2");font-weight:400 800}
    html,body{margin:0;width:${w}px;height:${h}px;overflow:hidden;background:#05060f}
    body{font-family:"JBM",ui-monospace,Menlo,monospace;color:#cfd8e6;position:relative}
    #bg{position:absolute;inset:0;
      background:
        radial-gradient(ellipse 80% 55% at 50% -10%, rgba(255,60,200,.22), transparent 70%),
        radial-gradient(ellipse 70% 60% at 50% 110%, rgba(52,231,255,.16), transparent 70%),
        #05060f}
    #grid{position:absolute;inset:0;opacity:.28;
      background-image:linear-gradient(rgba(52,231,255,.35) 1px, transparent 1px),
                       linear-gradient(90deg, rgba(52,231,255,.35) 1px, transparent 1px);
      background-size:${Math.round(92 * u)}px ${Math.round(92 * u)}px;
      -webkit-mask-image:linear-gradient(to bottom, transparent, #000 40%, #000 60%, transparent);
      mask-image:linear-gradient(to bottom, transparent, #000 40%, #000 60%, transparent)}
    ${layout}
    ${shotCss}
    h1{margin:0;font-size:${headFs}px;line-height:1.15;font-weight:800;letter-spacing:.14em;color:#eaffff;
       text-shadow:0 0 ${Math.round(10 * u)}px rgba(52,231,255,.9),0 0 ${Math.round(34 * u)}px rgba(52,231,255,.55),
                   0 0 ${Math.round(80 * u)}px rgba(52,231,255,.3)}
    p{margin:${Math.round(22 * u)}px 0 0;font-size:${subFs}px;line-height:1.35;font-weight:500;color:#ff5ce0;
      letter-spacing:.04em;text-shadow:0 0 ${Math.round(14 * u)}px rgba(255,92,224,.6)}
    #shot{border-radius:${radius}px;overflow:hidden;
      box-shadow:0 0 0 ${Math.round(3 * u)}px rgba(52,231,255,.55),0 0 ${Math.round(60 * u)}px rgba(52,231,255,.35),
                 0 ${Math.round(40 * u)}px ${Math.round(120 * u)}px rgba(0,0,0,.7)}
    #shot img{display:block;width:100%;height:100%;object-fit:cover;object-position:top}
  </style></head><body>
    <div id="bg"></div><div id="grid"></div>
    <div id="cap"><h1>${head}</h1><p>${sub}</p></div>
    <div id="shot"><img src="${img}"></div>
  </body></html>`;
}

const browser = await chromium.launch(
  process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {},
);
try {
  for (const set of fs.readdirSync(RAW).sort()) {
    if (process.env.SET && !set.startsWith(process.env.SET)) continue; // SET=iphone-6.5 → that set + its landscape
    const landscape = set.endsWith("-landscape");
    const outDir = path.join(FINAL, set);
    fs.rmSync(outDir, { recursive: true, force: true });
    fs.mkdirSync(outDir, { recursive: true });
    let n = 0;
    for (const [name, head, sub] of CAPTIONS) {
      const src = path.join(RAW, set, `${name}.png`);
      if (!fs.existsSync(src)) continue;
      const buf = fs.readFileSync(src);
      const w = buf.readUInt32BE(16), h = buf.readUInt32BE(20);
      const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
      await page.setContent(html({ img: png(src), w, h, landscape, head, sub }));
      await page.evaluate(async () => {
        await document.fonts.ready;
        const im = document.querySelector("img");
        if (!im.complete) await new Promise((r) => (im.onload = r));
      });
      const out = path.join(outDir, `${String(++n).padStart(2, "0")}-${name.replace(/^\d+-/, "")}.png`);
      await page.screenshot({ path: out, type: "png" });
      await page.close();
      console.log("  wrote", path.relative(ROOT, out), `${w}×${h}`);
    }
  }
} finally {
  await browser.close();
}
