// The web overlay over a real video window: does it work, and what does it cost.
//   node tools/overlay-check/gate.mjs all|work|monitors|files|rest|errors|tray|cost
// NEEDS A FREE SCREEN: it opens windows, moves the real mouse and clicks. Close your own Lumen session first:
// it stops if another window stays in front. Needs a release build; `tray` needs the service running.
import { spawn, spawnSync } from "node:child_process";
import { mkdirSync, rmSync, openSync, readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

const script = "C:/dev/lumen/tools/overlay-check";
const here = "C:/dev/lumen/bench-out/overlay-check";
const rel = "C:/dev/lumen/target/release";
const d = join(here, "gate");
const out = join(here, "shots");
const web = join(d, "wv");
const job = process.argv[2] ?? "all";
rmSync(d, { recursive: true, force: true });
for (const f of ["host", "hub", "wv"]) mkdirSync(join(d, f), { recursive: true });
mkdirSync(out, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const checks = [];
const check = (name, ok) => { checks.push(`${ok ? "ok  " : "FAIL"} ${name}`); console.log(checks.at(-1)); };
const win = (...a) => {
  const r = spawnSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(script, "win.ps1"), ...a.map(String)], { encoding: "utf8" });
  try { return JSON.parse(r.stdout); } catch { console.log("win.ps1:", r.stdout, r.stderr); return []; }
};
// Screen rectangles, [left, top, right, bottom].
const isInside = (r, c) => r[0] >= c[0] && r[1] >= c[1] && r[2] <= c[2] && r[3] <= c[3];
const rectOf = (r) => [r[0], r[1], r[2] - r[0], r[3] - r[1]].join(",");
const leftInDownloads = () => {
  try {
    return readdirSync(join(process.env.USERPROFILE, "Downloads", "Lumen")).filter((n) => n.startsWith("gate-"));
  } catch {
    return [];
  }
};
const db = join(d, "lumen.db");
const srv = (...a) => spawnSync(`${rel}/lumen-server.exe`, ["--db", db, ...a], { encoding: "utf8" });
srv("dev-cert", "--out", d);
const procs = [];
const start = (exe, args, name, env = {}, hide = true) => {
  const log = openSync(join(d, `${name}.log`), "a");
  const p = spawn(exe, args, { stdio: ["ignore", log, log], env: { ...process.env, ...env }, windowsHide: hide });
  procs.push(p);
  return p;
};
start(`${rel}/lumen-server.exe`, ["--db", db, "serve", "--listen", "127.0.0.1:7443", "--cert", join(d, "cert.pem"), "--key", join(d, "key.pem")], "server");
await sleep(600);
const ca = join(d, "ca.pem");
const server = ["--server", "127.0.0.1:7443", "--server-ca", ca];
const hostEnv = { LUMEN_DATA: join(d, "host") };
const token = spawnSync(`${rel}/lumen-host.exe`, ["token"], { encoding: "utf8", env: { ...process.env, ...hostEnv } }).stdout.trim();
let hostP = null;
const startHost = async (size, more = []) => {
  if (hostP) { hostP.kill(); await sleep(1500); }
  hostP = start(`${rel}/lumen-host.exe`, [...server, "--source", "synthetic", "--size", size, "--input", "dry-run", "--clipboard", "dry-run", "--audio", "off", ...more], "host", hostEnv);
  await sleep(1500);
};
await startHost("3840x2160");

// The hub, with a card ready; its own WebView2 data, with DevTools.
writeFileSync(join(d, "hub", "client-app.json"), JSON.stringify({ name: "gate", server: "127.0.0.1:7443", server_ca: ca, fps: 60, mbps: 0, borderless: false, shortcuts_to_host: true, cards: [{ id: 1, token, machine: "", account: "" }], next_id: 2 }));
const PORT = 9340;
const hubP = start(`${rel}/lumen-client-app.exe`, [], "hubproc", { LUMEN_DATA: join(d, "hub"), WEBVIEW2_USER_DATA_FOLDER: web, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}` }, false);

const targets = async () => { try { return await (await fetch(`http://127.0.0.1:${PORT}/json`)).json(); } catch { return []; } };
class Page {
  static async open(match, ms = 20000) {
    const t0 = Date.now();
    while (Date.now() - t0 < ms) {
      const t = (await targets()).find((x) => x.type === "page" && x.url.includes(match));
      if (t) {
        const p = new Page();
        p.ws = new WebSocket(t.webSocketDebuggerUrl);
        await new Promise((r) => (p.ws.onopen = r));
        p.id = 0; p.waiting = new Map(); p.errors = [];
        p.ws.onmessage = (m) => {
          const e = JSON.parse(m.data);
          if (e.id) { p.waiting.get(e.id)?.(e); p.waiting.delete(e.id); }
          if (e.method === "Runtime.exceptionThrown") p.errors.push(e.params.exceptionDetails.exception?.description ?? e.params.exceptionDetails.text);
          if (e.method === "Log.entryAdded" && e.params.entry.level === "error") p.errors.push(e.params.entry.text);
        };
        await p.send("Runtime.enable"); await p.send("Log.enable");
        return p;
      }
      await sleep(200);
    }
    return null;
  }
  send(method, params = {}) { return new Promise((r) => { const i = ++this.id; this.waiting.set(i, r); this.ws.send(JSON.stringify({ id: i, method, params })); }); }
  async ev(expression) { return (await this.send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result?.result?.value; }
  async until(what, expr, ms = 15000) { const t = Date.now(); while (Date.now() - t < ms) { if (await this.ev(expr)) return true; await sleep(150); } console.log("timed out:", what); return false; }
}
const invoke = (p, cmd, args) => p.ev(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args ?? {})}).then(()=>true,e=>String(e))`);

const WM_SHORTCUT = 0x8002;
const key = (hwnd, c) => win("post", hwnd, WM_SHORTCUT, c.charCodeAt(0));
const videoProc = () => {
  const r = spawnSync("powershell", ["-NoProfile", "-Command", `(Get-CimInstance Win32_Process -Filter "Name='lumen-client.exe'" | Where-Object { $_.ExecutablePath -like '${rel.replace(/\//g, "\\")}*' }).ProcessId`], { encoding: "utf8" });
  return r.stdout.trim().split(/\s+/).filter(Boolean).map(Number);
};
const newestLog = () => {
  const dir = join(d, "hub", "logs");
  const f = readdirSync(dir).filter((n) => n.startsWith("client-")).map((n) => join(dir, n)).sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
  return readFileSync(f, "utf8");
};
const presented = (log) => { const m = log.match(/capture -> presented\s+ms: p50\s+([\d.]+)\s+p95\s+([\d.]+)/); return m ? [Number(m[1]), Number(m[2])] : null; };
const frames = (log) => { const m = log.match(/frames presented: (\d+)/); return m ? Number(m[1]) : null; };

let main, summary = {};
try {
  main = await Page.open("index.html") ?? await Page.open("tauri.localhost");
  check("the hub's page opens", !!main);
  await main.until("the page", `!!window.__TAURI_INTERNALS__`);
  await sleep(2500);
  console.log("request:", await invoke(main, "request_access", { id: 1 }));
  await sleep(800);
  const row = srv("access", "--status", "pending").stdout.split("\n")[0] ?? "";
  const m = row.match(/client\s+(\d+).*host\s+(\d+)/);
  check(`a request to approve: ${row.trim()}`, !!m);
  srv("approve", m[1], m[2]);
  await sleep(2500);

  const hubSession = async () => {
    console.log("connect:", await invoke(main, "connect", { id: 1, title: "Lumen — gate" }));
    let pid, video;
    for (let i = 0; i < 100 && !video; i++) {
      await sleep(200);
      pid = videoProc()[0];
      if (pid) video = win("windows", pid).find((w) => w.visible && w.cls === "LumenClientWindow");
    }
    // Real input and screenshots follow: only over our own window.
    if (video) {
      win("front", video.hwnd);
      await sleep(500);
      const fg = win("state", video.hwnd)[0].fg;
      if (fg !== video.hwnd) throw new Error(`another window is in front (${fg}): close it, the check does not go on over it`);
    }
    return { pid, video };
  };

  // ---- Does it work
  if (job === "all" || job === "work") {
    const { pid, video } = await hubSession();
    check("the video window opens", !!video);
    win("front", video.hwnd);
    let page = await Page.open("overlay.html");
    check("the overlay page opens", !!page);
    const hubWins = () => win("windows", hubP.pid);
    const ov = () => hubWins().find((w) => w.title === "Lumen Overlay");
    const ovw = ov();
    check("its window exists, hidden while there is nothing to show", !!ovw && !ovw.visible);
    await sleep(4000);
    // Hidden, the window already lies over the picture: the page is laid out at its size before it shows (no jump).
    const hidden = win("state", ovw.hwnd)[0];
    const pageWidth = await page.ev(`Math.round(innerWidth * devicePixelRatio)`);
    console.log("hidden overlay", JSON.stringify(hidden), "page width", pageWidth);
    check(`hidden, already the picture's size: page ${pageWidth} px, window ${hidden.rect}`, Math.abs(pageWidth - (hidden.rect[2] - hidden.rect[0])) <= 1 && hidden.rect[2] - hidden.rect[0] > 900);
    // The block's size the moment it appears, against a moment later.
    await page.ev(`(()=>{window.__first=null;new MutationObserver((_, o)=>{const e=document.querySelector('.stats');if(e){const r=e.getBoundingClientRect();window.__first=[r.width,r.height,innerWidth];o.disconnect()}}).observe(document.body,{childList:true,subtree:true});return true})()`);
    key(video.hwnd, "O");
    check("Ctrl+Alt+Shift+O: the numbers come", await page.until("stats", `!!document.querySelector('.stats')`, 5000));
    await sleep(2500);
    const sizes = await page.ev(`(()=>{const r=document.querySelector('.stats').getBoundingClientRect();return {first:window.__first,later:[r.width,r.height,innerWidth]}})()`);
    console.log("the block's size, first and later:", JSON.stringify(sizes));
    check("the block appears at its size, no jump", !!sizes.first && Math.abs(sizes.first[0] - sizes.later[0]) <= 1 && sizes.first[2] === sizes.later[2]);
    const st = win("state", ovw.hwnd)[0];
    const vs = win("state", video.hwnd)[0];
    console.log("overlay", JSON.stringify(st), "video", JSON.stringify(vs));
    check("the window is shown", st.visible);
    check(`over the picture: ${st.rect} in ${vs.client}`, isInside(st.rect, vs.client));
    const EX = { TRANSPARENT: 0x20, LAYERED: 0x80000, NOACTIVATE: 0x8000000, TOOLWINDOW: 0x80, APPWINDOW: 0x40000 };
    check("clicks go through, never the focus, no taskbar button", (st.ex & EX.TRANSPARENT) && (st.ex & EX.LAYERED) && (st.ex & EX.NOACTIVATE) && (st.ex & EX.TOOLWINDOW) && !(st.ex & EX.APPWINDOW));
    check("the focus stayed with the video window", vs.fg === video.hwnd);
    const block = await page.ev(`(()=>{const r=document.querySelector('.stats').getBoundingClientRect(),k=devicePixelRatio;return [r.left*k,r.top*k,r.width*k,r.height*k,k]})()`);
    const pt = [Math.round(st.rect[0] + block[0] + block[2] / 2), Math.round(st.rect[1] + block[1] + block[3] / 2)];
    const hit = win("frompoint", pt[0], pt[1])[0];
    check(`a point on the block belongs to the video window: ${JSON.stringify(hit)}`, hit.hwnd === video.hwnd);
    win("shot", join(out, "gate-stats.png"), rectOf(vs.client));
    const stats = await page.ev(`document.querySelector('.stats').innerText`);
    console.log("--- the block says:\n" + stats + "\n---");
    check("real numbers", /Задержка\s+\d+ мс/.test(stats) && /из 60 кадров/.test(stats) && /HEVC 10 бит 4:2:0/.test(stats));
    check("three graphs, each with its line", await page.ev(`(()=>{const c=[...document.querySelectorAll('.chart')];return c.length===3&&c.every(x=>x.querySelector('polyline')?.getAttribute('points')?.length>100)})()`));

    // Following: another window size
    const before = st.rect.join();
    key(video.hwnd, "F");
    await sleep(1500);
    const st2 = win("state", ovw.hwnd)[0], vs2 = win("state", video.hwnd)[0];
    check(`borderless on the go: follows ${before} -> ${st2.rect}`, st2.visible && st2.rect.join() !== before && st2.rect[2] <= vs2.client[2]);
    win("shot", join(out, "gate-borderless.png"), rectOf(vs2.client));
    key(video.hwnd, "F");
    await sleep(1500);

    // Z-order: another window in front, then back
    win("front", hubP ? video.hwnd : 0);
    key(video.hwnd, "O");
    check("the same key: gone", await page.until("no stats", `!document.querySelector('.stats')`, 3000));
    await sleep(600);
    check("nothing to show: the window is hidden (no composition over the video)", !win("state", ovw.hwnd)[0].visible);
    console.log("page errors:", page.errors.length, page.errors.slice(0, 3));
    check("no page errors", page.errors.length === 0);
    key(video.hwnd, "Q");
    await sleep(1500);
    check("the session ends: the overlay's window goes", !ov());
    check("the hub comes back", hubWins().some((w) => w.title === "Lumen Client" && w.visible));
  }

  // ---- Several monitors: the circles, real clicks
  if (job === "all" || job === "monitors") {
    await startHost("1920x1080,1600x900,1280x720");
    await sleep(2500);
    const { video } = await hubSession();
    check("session with three monitors", !!video);
    win("front", video.hwnd);
    await sleep(3000);
    const page = await Page.open("overlay.html");
    key(video.hwnd, "L");
    check("mode 2: circles", await page.until("circles", `document.querySelectorAll('.dot').length===3`, 6000));
    await sleep(700);
    const ovw = win("windows", hubP.pid).find((w) => w.title === "Lumen Overlay");
    const st = win("state", ovw.hwnd)[0];
    check("the window stays over the picture in mode 2", st.visible);
    const dot = await page.ev(`(()=>{const r=document.querySelectorAll('.dot')[2].getBoundingClientRect(),k=devicePixelRatio;return [(r.left+r.width/2)*k,(r.top+r.height/2)*k]})()`);
    const away = [st.rect[0] + 30, st.rect[3] - 30];
    check("away from the circles clicks go through", win("frompoint", ...away)[0].hwnd === video.hwnd);
    const at = [Math.round(st.rect[0] + dot[0]), Math.round(st.rect[1] + dot[1])];
    const before = await page.ev(`document.querySelector('.dot.on').textContent`);
    console.log("dot", JSON.stringify(dot), "window", JSON.stringify(st.rect), "dpr", await page.ev("devicePixelRatio"), "inner", await page.ev("[innerWidth,innerHeight]"));
    check("the overlay is really above the video window", win("above", ovw.hwnd, video.hwnd)[0].above);
    win("shot", join(out, "gate-mode2-before.png"), rectOf(st.rect));
    const c = win("click", at[0], at[1], video.hwnd)[0];
    console.log("click", JSON.stringify(c), "overlay", ovw.hwnd, "video", video.hwnd);
    check("under the mouse: the overlay takes the click", c.under === ovw.hwnd);
    check("the focus stays with the video window", c.fg === video.hwnd);
    check(`the click shows monitor 3 (was ${before})`, await page.until("monitor 3", `document.querySelector('.dot.on')?.textContent==='III'`, 4000));
    win("shot", join(out, "gate-mode2.png"), rectOf(st.rect));
    await sleep(600);
    const ex = win("state", ovw.hwnd)[0].ex;
    check("the mouse gone: clicks go through again", (ex & 0x20) !== 0);
    key(video.hwnd, "L");
    check("mode 3: no circles", await page.until("no circles", `document.querySelectorAll('.dot').length===0`, 6000));
    key(video.hwnd, "L");
    await sleep(1500);
    key(video.hwnd, "Q");
    await sleep(2000);
    await startHost("3840x2160");
    await sleep(2500);
  }

  // ---- Real transfers: two files from the host, slowly; «Отменить всё» with a real click. Nothing stays in Downloads\Lumen.
  if (job === "all" || job === "files") {
    const names = ["gate-a.bin", "gate-b.bin"].map((n) => join(d, n));
    for (const n of names) writeFileSync(n, Buffer.alloc(300 * 1024 * 1024));
    await startHost("1920x1080", ["--file-mbps", "20", ...names.flatMap((n) => ["--send-file", n])]);
    await sleep(2500);
    const { video } = await hubSession();
    check("session with two files on the way", !!video);
    win("front", video.hwnd);
    const page = await Page.open("overlay.html");
    check("downloads bring the block up by themselves", await page.until("rows", `document.querySelectorAll('.transfer').length===2`, 15000));
    check("the clipboard's line is there, empty", await page.ev(`document.querySelector('.clipboard').innerText.includes('Файлов нет')`));
    await sleep(1500);
    const ovw = win("windows", hubP.pid).find((w) => w.title === "Lumen Overlay");
    const st = win("state", ovw.hwnd)[0];
    win("shot", join(out, "gate-transfers.png"), rectOf(st.rect));
    const button = await page.ev(`(()=>{const b=document.querySelector('.transfers .block-head button');if(!b)return null;const r=b.getBoundingClientRect(),k=devicePixelRatio;return [(r.left+r.width/2)*k,(r.top+r.height/2)*k]})()`);
    check("«Отменить всё» is there", !!button);
    if (button) {
      const at = [Math.round(st.rect[0] + button[0]), Math.round(st.rect[1] + button[1])];
      const c = win("click", at[0], at[1], video.hwnd)[0];
      check("the block takes the click, the focus stays", c.under === ovw.hwnd && c.fg === video.hwnd);
      const seen = [];
      for (let i = 0; i < 25; i++) {
        const says = await page.ev(`[...document.querySelectorAll('.transfer .transfer-state')].map(e=>e.textContent).join(' | ')`);
        if (seen.at(-1)?.[1] !== says) seen.push([i * 300, says]);
        await sleep(300);
      }
      console.log("after the click, ms:", JSON.stringify(seen));
      check("both cancelled, nothing goes", seen.some(([, t]) => t.split(" | ").length === 2 && t.split(" | ").every((x) => x.includes("отменено"))));
      check("the button is gone", await page.until("no button", `!document.querySelector('.transfers .block-head button')`, 3000));
      win("shot", join(out, "gate-transfers-cancelled.png"), rectOf(st.rect));
    }
    console.log("page errors:", page.errors.length, page.errors.slice(0, 3));
    check("no page errors", page.errors.length === 0);
    key(video.hwnd, "Q");
    await sleep(2000);
    const left = leftInDownloads();
    check(`nothing left in the downloads: ${left}`, left.length === 0);
    await startHost("3840x2160");
    await sleep(2500);
  }

  // ---- What is left: another window over the video and back; mode 1 with both blocks; mode 3 and the notice.
  if (job === "all" || job === "rest") {
    const file = join(d, "gate-rest.bin");
    writeFileSync(file, Buffer.alloc(300 * 1024 * 1024));
    await startHost("1920x1080,1600x900,1280x720", ["--file-mbps", "10", "--send-file", file]);
    await sleep(2500);
    const { video } = await hubSession();
    check("session with three monitors and a file on the way", !!video);
    win("front", video.hwnd);
    const page = await Page.open("overlay.html");
    await sleep(3000);
    key(video.hwnd, "O");
    check("mode 1: the numbers and the transfers", await page.until("both", `!!document.querySelector('.stats')&&!!document.querySelector('.transfer')`, 15000));
    await sleep(1500);
    const ovw = win("windows", hubP.pid).find((w) => w.title === "Lumen Overlay");
    let st = win("state", ovw.hwnd)[0];
    let vs = win("state", video.hwnd)[0];
    check(`mode 1: the window is over the picture: ${st.rect} in ${vs.client}`, isInside(st.rect, vs.client));
    const stage = await page.ev(`(()=>{const r=document.querySelector('.stage').getBoundingClientRect();return [r.left,r.top,r.right,r.bottom,innerWidth,innerHeight]})()`);
    check(`mode 1: the blocks are on monitor 1, not across the layout: ${JSON.stringify(stage)}`, stage[0] <= 1 && stage[1] <= 1 && stage[2] < stage[4] * 0.6);
    const blocks = await page.ev(`(()=>{const q=(s)=>{const r=document.querySelector(s).getBoundingClientRect();return [r.left,r.top,r.right,r.bottom]};return {a:q('.stats'),b:q('.transfers'),w:[innerWidth,innerHeight]}})()`);
    const { a, b, w } = blocks;
    check(`the two blocks keep apart, inside the window: ${JSON.stringify(blocks)}`, (a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1]) && [a, b].every((r) => r[0] >= 0 && r[1] >= 0 && r[2] <= w[0] + 1 && r[3] <= w[1] + 1));
    win("shot", join(out, "gate-mode1.png"), rectOf(st.rect));

    // Another window over the video, and back
    const TOPMOST = 8;
    const isAbove = (x, y) => win("above", x, y)[0].above;
    check("in front: the overlay is topmost, above the video window", (st.ex & TOPMOST) !== 0 && isAbove(ovw.hwnd, video.hwnd));
    const k = await page.ev("devicePixelRatio");
    const cx = Math.round(st.rect[0] + ((a[0] + a[2]) / 2) * k);
    const coverP = start("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(script, "cover.ps1"), cx - 500, vs.client[3] - 500, 1000, 480].map(String), "cover");
    let cover;
    for (let i = 0; i < 40 && !cover; i++) {
      await sleep(250);
      cover = win("windows", coverP.pid).find((x) => x.title === "Lumen Gate Cover");
    }
    check("the other window is up", !!cover);
    await sleep(500);
    const onCover = [cx, vs.client[3] - 250];
    const c1 = win("clickon", onCover[0], onCover[1], cover.hwnd, video.hwnd)[0];
    console.log("click on the other window", JSON.stringify(c1));
    check("the other window takes the click and the focus", c1.fg === cover.hwnd);
    await sleep(500);
    st = win("state", ovw.hwnd)[0];
    check("it in front: the overlay is no longer topmost", (st.ex & TOPMOST) === 0);
    check("order: it, the overlay, the video window", isAbove(cover.hwnd, ovw.hwnd) && isAbove(ovw.hwnd, video.hwnd));
    check("the overlay stays shown", st.visible);
    win("shot", join(out, "gate-covered.png"), rectOf(st.rect));
    await sleep(1000);
    check("the overlay does not take the focus back", win("state", video.hwnd)[0].fg === cover.hwnd);
    const onVideo = [vs.client[0] + 40, vs.client[1] + 40];
    const c2 = win("clickon", onVideo[0], onVideo[1], video.hwnd, cover.hwnd)[0];
    console.log("click on the video window", JSON.stringify(c2));
    check("back on the video window", c2.fg === video.hwnd);
    await sleep(500);
    st = win("state", ovw.hwnd)[0];
    check("in front again: topmost, above the other window", (st.ex & TOPMOST) !== 0 && isAbove(ovw.hwnd, cover.hwnd));
    win("shot", join(out, "gate-back.png"), rectOf(st.rect));
    coverP.kill();
    await sleep(500);

    // Mode 3: only over the main window; two windows on one monitor: the notice
    key(video.hwnd, "L");
    await sleep(1500);
    key(video.hwnd, "L");
    await sleep(3000);
    const wins = win("windows", videoProc()[0]).filter((x) => x.visible && x.cls === "LumenClientWindow");
    check(`mode 3: three windows (${wins.length})`, wins.length === 3);
    vs = win("state", video.hwnd)[0];
    st = win("state", ovw.hwnd)[0];
    check(`mode 3: the overlay only over the main window's picture: ${st.rect} in ${vs.client}`, isInside(st.rect, vs.client));
    check("mode 3: one overlay window", win("windows", hubP.pid).filter((x) => x.title === "Lumen Overlay").length === 1);
    check("mode 3: no circles", await page.ev(`document.querySelectorAll('.dot').length===0`));
    win("front", video.hwnd);
    await sleep(500);
    const others = wins.filter((x) => x.hwnd !== video.hwnd);
    check("mode 3: the other monitors' windows stay above the overlay, or it is right over the main window", others.every((x) => isAbove(x.hwnd, ovw.hwnd) || !isAbove(x.hwnd, video.hwnd)) && isAbove(ovw.hwnd, video.hwnd));
    win("shot", join(out, "gate-mode3.png"), rectOf(st.rect));
    key(video.hwnd, "F");
    check("two windows on one monitor: the notice", await page.until("notice", `!!document.querySelector('.notice')`, 5000));
    await sleep(300);
    check("the notice does not cover the numbers", await page.ev(`(()=>{const n=document.querySelector('.notice').getBoundingClientRect(),s=document.querySelector('.stats').getBoundingClientRect();return n.bottom<=s.top||s.bottom<=n.top})()`));
    console.log("notice:", await page.ev(`document.querySelector('.notice')?.textContent`));
    st = win("state", ovw.hwnd)[0];
    win("shot", join(out, "gate-notice.png"), rectOf(st.rect));
    check("the notice goes by itself", await page.until("no notice", `!document.querySelector('.notice')`, 7000));
    console.log("page errors:", page.errors.length, page.errors.slice(0, 3));
    check("no page errors", page.errors.length === 0);
    key(video.hwnd, "Q");
    await sleep(2500);
    const left = leftInDownloads();
    check(`nothing left in the downloads: ${left}`, left.length === 0);
    await startHost("3840x2160");
    await sleep(2500);
  }

  // ---- What the core reports: its codes, in words, in the real hub's dialog. No mouse: the page's own controls.
  if (job === "all" || job === "errors") {
    const raw = async (cmd, args) => main.ev(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(cmd)}, ${JSON.stringify(args)}).then(()=>"ok",e=>String(e))`);
    check("the core sends a code: an address", (await raw("set_identity", { name: "gate", server: "a b" })) === "err_server_address");
    check("the core sends a code: the server stays with machines", (await raw("set_identity", { name: "gate", server: "127.0.0.2:7443" })) === "err_server_has_machines");
    check("the core sends a code: a token", (await raw("add_card", { token: "nope" })) === "err_token");
    check("the core sends a code: the card exists", (await raw("add_card", { token })) === "err_card_exists");
    const words = async (tokenText) => {
      await main.ev(`[...document.querySelectorAll('.tabs button')][0].click()`);
      await sleep(300);
      await main.ev(`[...document.querySelectorAll('button')].find(b=>b.textContent.includes('Добавить')).click()`);
      await sleep(400);
      await main.ev(`(()=>{const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;const [a]=document.querySelectorAll('.dialog .field input');set.call(a,${JSON.stringify(tokenText)});a.dispatchEvent(new Event('input',{bubbles:true}))})()`);
      await sleep(200);
      await main.ev(`document.querySelector('.dialog button.primary, [role=dialog] button.primary, .btn.primary:not(.machine-actions .btn)').click()`);
      await main.until("the error", `!!document.querySelector('.dialog-error')`, 5000);
      const text = await main.ev(`document.querySelector('.dialog-error')?.textContent`);
      await main.ev(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='Отмена')?.click()`);
      await sleep(300);
      return text;
    };
    const t = await words("nope");
    console.log("dialog says:", t);
    check("the dialog has the words: the token", t === "Токен: неверный формат (вида XXXX-XXXX-XXXX-XXXX-XXXX-XXXX)");
    const d = await words(token);
    console.log("dialog says:", d);
    check("the dialog has the words: the same card", d === "Такая карточка уже есть");
    win("shot", join(out, "gate-errors.png"), "0,0,1600,900");

    // The real button of the row: the hub makes the title and passes it on.
    await main.ev(`[...document.querySelectorAll('.machine .btn.primary')].find(b=>b.textContent.includes('Подключиться')).click()`);
    let video;
    for (let i = 0; i < 100 && !video; i++) {
      await sleep(200);
      const pid = videoProc()[0];
      if (pid) video = win("windows", pid).find((w) => w.visible && w.cls === "LumenClientWindow");
    }
    console.log("video window:", video?.title);
    check("the video window's title comes from the hub", /^Lumen — /.test(video?.title ?? ""));
    if (video) key(video.hwnd, "Q");
    await sleep(1500);
  }

  // ---- The tray's menu: its words come from the page, in a hidden window too (as at sign-in). Needs the service running:
  // otherwise the app would start it, or quit at once when hidden.
  if (job === "tray") {
    const running = spawnSync("powershell", ["-NoProfile", "-Command", "(Get-Service LumenHost -ErrorAction SilentlyContinue).Status"], { encoding: "utf8" }).stdout.trim();
    check(`the service runs: ${running}`, running === "Running");
    const others = spawnSync("powershell", ["-NoProfile", "-Command", "(Get-Process lumen-host-app -ErrorAction SilentlyContinue).Id"], { encoding: "utf8" }).stdout.trim();
    check("no other Lumen Host is open", others === "");
    if (running === "Running" && others === "") {
      for (const [name, args] of [["hidden", ["--hidden"]], ["shown", []]]) {
        const data = join(d, `hostapp-${name}`);
        mkdirSync(data, { recursive: true });
        const p = start(`${rel}/lumen-host-app.exe`, args, `hostapp-${name}`, { LUMEN_DATA: data }, false);
        let set = false;
        for (let i = 0; i < 60 && !set; i++) {
          await sleep(250);
          try {
            const dir = join(data, "logs");
            set = readdirSync(dir).some((n) => readFileSync(join(dir, n), "utf8").includes("the tray's menu is set"));
          } catch {}
        }
        check(`${name}: the page set the tray's menu`, set);
        const w = win("windows", p.pid).find((x) => x.title === "Lumen Host");
        check(`${name}: the window is ${name}`, !!w && w.visible === (name === "shown"));
        p.kill();
        await sleep(1000);
      }
    }
  }

  // ---- What it costs: the overlay on / none, in turns
  if (job === "all" || job === "cost") {
    const results = { web: [], off: [] };
    const SECONDS = 14;
    for (let round = 0; round < 3; round++) {
      for (const kind of ["web", "off"]) {
        const { video } = await hubSession();
        win("front", video.hwnd);
        if (kind === "web") { await sleep(1500); key(video.hwnd, "O"); }
        let cpu0, cpu1;
        await sleep(2000);
        if (round === 0 && kind === "web") cpu0 = win("cpu", web.replace(/\//g, "\\"));
        await sleep((SECONDS - 4) * 1000);
        if (round === 0 && kind === "web") {
          cpu1 = win("cpu", web.replace(/\//g, "\\"));
          const secs = SECONDS - 4;
          const use = cpu1.reduce((a, x) => a + x.cpu, 0) - cpu0.reduce((a, x) => a + x.cpu, 0);
          summary.webviewCpu = `${((100 * use) / secs).toFixed(1)} % of a core over ${secs} s`;
          summary.webviewMb = cpu1.reduce((a, x) => a + x.mb, 0);
          summary.webviewProcs = cpu1.map((x) => `${x.name} ${x.mb} MB`).join(", ");
        }
        key(video.hwnd, "Q");
        await sleep(1500);
        const log = newestLog();
        results[kind].push(presented(log));
        console.log(kind, round, presented(log), frames(log));
        await sleep(800);
      }
    }
    summary.results = results;
    const med = (l) => { const v = l.filter(Boolean).map((x) => x[0]).sort((a, b) => a - b); return v[Math.floor(v.length / 2)]; };
    const p95 = (l) => { const v = l.filter(Boolean).map((x) => x[1]).sort((a, b) => a - b); return v[Math.floor(v.length / 2)]; };
    console.log(`capture -> presented, median of runs: web p50 ${med(results.web)} p95 ${p95(results.web)} | none p50 ${med(results.off)} p95 ${p95(results.off)}`);
    console.log("webview:", JSON.stringify(summary));
  }
} catch (e) {
  console.log("ERROR", e);
} finally {
  console.log(`${checks.filter((c) => c.startsWith("ok")).length} ok, ${checks.filter((c) => c.startsWith("FAIL")).length} failed`);
  for (const pid of videoProc()) { try { process.kill(pid); } catch {} }
  for (const p of procs) { try { p.kill(); } catch {} }
  await sleep(800);
  process.exit(0);
}
