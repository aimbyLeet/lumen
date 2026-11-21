// Every scene of the interfaces' stand-ins, in a headless browser with the clock stopped: compare before and after a
// refactoring, byte for byte. Does not touch the screen or the mouse.
//   node tools/overlay-check/snapshots.mjs <out dir> [--lang ru]   (the interfaces' language; default English, not the browser's)
//   node tools/overlay-check/snapshots.mjs compare <dir a> <dir b>
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

if (process.argv[2] === "compare") {
  const [a, b] = process.argv.slice(3);
  const hash = (f) => createHash("sha256").update(readFileSync(f)).digest("hex");
  const names = new Set([...readdirSync(a), ...readdirSync(b)]);
  const differ = [...names].filter((n) => {
    try {
      return hash(join(a, n)) !== hash(join(b, n));
    } catch {
      return true;
    }
  });
  console.log(`${names.size} snapshots, ${differ.length} differ`);
  for (const n of differ) console.log("  " + n);
  process.exit(differ.length ? 1 : 0);
}

const out = process.argv[2];
const langAt = process.argv.indexOf("--lang");
/** Added to every page's address; the snapshots' names stay the same, so that languages compare with the old ones. */
const langQuery = `&lang=${langAt > 0 ? process.argv[langAt + 1] : "en"}`;
const web = "C:/dev/lumen/web";
const edge = "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe";
const PORT = 9351;
const profile = join(process.env.TEMP, `lumen-snap-${process.pid}`);
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// The clock stands still at one moment, for everything on the page.
const STOP_CLOCK = `(() => {
  const at = new Date("2026-11-02T12:00:00+05:00").getTime();
  const Real = Date;
  class Stopped extends Real {
    constructor(...a) { if (a.length) super(...a); else super(at); }
    static now() { return at; }
  }
  globalThis.Date = Stopped;
  const s = document.createElement("style");
  s.textContent = "*,*::before,*::after{animation:none!important;transition:none!important;caret-color:transparent!important}";
  addEventListener("DOMContentLoaded", () => document.head.append(s));
})();`;

/** The keys of the stand-in's SCENES: a key at the start of a line of its body. */
function scenes(file, marker) {
  const s = readFileSync(file, "utf8").replaceAll("\r\n", "\n");
  const start = s.indexOf(marker);
  const body = s.slice(start, s.indexOf("\n};", start));
  return [...body.matchAll(/^ {2}"?([\w-]+)"?: \{/gm)].map((m) => m[1]);
}

const apps = [
  { name: "client", port: 5181, pages: scenes(`${web}/client/src/mock/core.ts`, "export const SCENES").map((s) => `/?scene=${s}`) },
  { name: "overlay", dir: "client", port: 5182, pages: scenes(`${web}/client/src/mock/event.ts`, "export const OVERLAY_SCENES").map((s) => `/overlay.html?scene=${s}`) },
  { name: "host", port: 5183, pages: scenes(`${web}/host/src/mock/core.ts`, "export const SCENES").flatMap((s) => [`/?scene=${s}`, `/?scene=${s}&tab=monitors`]) },
  { name: "admin", port: 5184, pages: scenes(`${web}/admin/src/mock/core.ts`, "export const SCENES").map((s) => `/admin/?scene=${s}`) },
];

const browser = spawn(edge, ["--headless=new", "--disable-gpu", "--hide-scrollbars", `--remote-debugging-port=${PORT}`, `--user-data-dir=${profile}`, "--window-size=1920,1080", "about:blank"], { stdio: "ignore", windowsHide: true });

async function connect() {
  for (let i = 0; i < 80; i++) {
    try {
      const t = (await (await fetch(`http://127.0.0.1:${PORT}/json`)).json()).find((x) => x.type === "page");
      if (t) return t.webSocketDebuggerUrl;
    } catch {}
    await sleep(250);
  }
  throw new Error("no browser");
}

const ws = new WebSocket(await connect());
await new Promise((r) => (ws.onopen = r));
let id = 0;
const waiting = new Map();
ws.onmessage = (m) => {
  const e = JSON.parse(m.data);
  if (e.id) {
    waiting.get(e.id)?.(e.result);
    waiting.delete(e.id);
  }
};
const send = (method, params = {}) =>
  new Promise((r) => {
    const i = ++id;
    waiting.set(i, r);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
await send("Page.enable");
await send("Page.addScriptToEvaluateOnNewDocument", { source: STOP_CLOCK });
await send("Emulation.setDeviceMetricsOverride", { width: 1920, height: 1080, deviceScaleFactor: 1, mobile: false });

let total = 0;
for (const app of apps) {
  const server = spawn("cmd.exe", ["/c", `npx vite --mode mock --port ${app.port} --strictPort`], { cwd: `${web}/${app.dir ?? app.name}`, stdio: "ignore", windowsHide: true });
  try {
    for (let i = 0; i < 80; i++) {
      try {
        await fetch(`http://localhost:${app.port}/`);
        break;
      } catch {
        await sleep(250);
      }
    }
    for (const page of app.pages) {
      await send("Page.navigate", { url: `http://localhost:${app.port}${page}${langQuery}` });
      // The stand-ins answer at once; the first page of a server takes longer to transform.
      await sleep(total && page !== app.pages[0] ? 700 : 3000);
      const snap = async (suffix = "") => {
        const shot = await send("Page.captureScreenshot", { format: "png" });
        writeFileSync(join(out, `${app.name}-${page.replace(/[^\w-]+/g, "_")}${suffix}.png`), Buffer.from(shot.data, "base64"));
        total++;
      };
      await snap();
      // Every tab; in the admin's lists every block opened: what a closed block hides is drawn too.
      const tabs = (await send("Runtime.evaluate", { expression: "document.querySelectorAll('.tabs button').length", returnByValue: true })).result.value ?? 0;
      if (app.name !== "overlay") {
        for (let t = 1; t < tabs; t++) {
          await send("Runtime.evaluate", { expression: `document.querySelectorAll('.tabs button')[${t}]?.click()` });
          await sleep(250);
          await snap(`-tab${t}`);
        }
      }
      if (app.name === "admin") {
        for (let t = 0; t < 2 && t < tabs; t++) {
          await send("Runtime.evaluate", { expression: `document.querySelectorAll('.tabs button')[${t}]?.click()` });
          await sleep(200);
          await send("Runtime.evaluate", { expression: "document.querySelectorAll('.entry:not(.open) .entry-head').forEach((b) => b.click())" });
          await sleep(250);
          await snap(`-open${t}`);
        }
      }
    }
  } finally {
    spawnSync("taskkill", ["/PID", String(server.pid), "/T", "/F"], { windowsHide: true });
  }
  console.log(app.name, app.pages.length);
}
browser.kill();
await sleep(500);
spawnSync("taskkill", ["/PID", String(browser.pid), "/T", "/F"], { windowsHide: true });
rmSync(profile, { recursive: true, force: true });
console.log("snapshots:", total);
process.exit(0);
