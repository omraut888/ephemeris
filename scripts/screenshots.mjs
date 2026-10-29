#!/usr/bin/env node
// Screenshots of the web app with headless Chrome, for the README and for
// checking light mode and phone width (docs/phase2-design.md §6).
//
//   node scripts/screenshots.mjs                 # the README set → docs/img/
//   node scripts/screenshots.mjs --all --out DIR # every combination, to review
//   node scripts/screenshots.mjs --build         # rebuild web/dist first
//
// It serves web/dist with `vite preview` on a free port, drives Chrome over
// the DevTools protocol (light/dark via emulated prefers-color-scheme, phone
// via device metrics), and stops both when done. Needs Node 22+ (global
// WebSocket), the web app's npm dependencies, a build in web/dist (run
// `cd web && npm run build`, or pass --build), and Chrome: set CHROME to its
// binary if it is not in the usual place. The sandbox shots load
// data/sandbox/feed.json (written by scripts/fetch_plaid_sandbox.py, not
// committed); without it they are skipped. Fails on any page error.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const WEB = join(ROOT, "web");
const SANDBOX_FEED = join(ROOT, "data/sandbox/feed.json");

const DEVICES = {
  desktop: { width: 1440, height: 900, mobile: false },
  phone: { width: 390, height: 844, mobile: true },
};

/** The README set: one dimension varied per shot where possible. */
const README_SET = [
  { source: "demo", theme: "dark", device: "desktop", view: "daily" },
  { source: "sandbox", theme: "dark", device: "desktop", view: "daily" },
  { source: "sandbox", theme: "light", device: "desktop", view: "lows" },
  { source: "demo", theme: "light", device: "phone", view: "daily" },
  { source: "sandbox", theme: "dark", device: "phone", view: "lows" },
];

function allShots() {
  const shots = [];
  for (const source of ["demo", "sandbox"])
    for (const theme of ["dark", "light"])
      for (const device of ["desktop", "phone"])
        for (const view of ["daily", "lows"]) shots.push({ source, theme, device, view });
  return shots;
}

const name = (s) => `${s.source}-${s.device}-${s.theme}-${s.view}.png`;

function parseArgs(argv) {
  const args = { all: false, build: false, out: join(ROOT, "docs/img") };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--all") args.all = true;
    else if (a === "--build") args.build = true;
    else if (a === "--out") args.out = resolve(argv[++i] ?? "");
    else throw new Error(`unknown argument ${a} (see the header of this file)`);
  }
  return args;
}

function findChrome() {
  const candidates = [
    process.env.CHROME,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ].filter(Boolean);
  const found = candidates.find((p) => existsSync(p));
  if (!found) throw new Error("Chrome not found: set CHROME to its binary");
  return found;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Stop a child and wait (up to 5 s) until it has exited. */
function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  const exited = new Promise((r) => child.once("exit", r));
  child.kill();
  return Promise.race([exited, sleep(5000)]);
}

function freePort() {
  return new Promise((res, rej) => {
    const srv = createServer();
    srv.unref();
    srv.on("error", rej);
    srv.listen(0, "127.0.0.1", () => {
      const { port } = srv.address();
      srv.close(() => res(port));
    });
  });
}

async function waitFor(what, test, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    try {
      if (await test()) return;
    } catch {
      // not up yet
    }
    await sleep(150);
  }
  throw new Error(`timed out waiting for ${what}`);
}

/** A minimal DevTools-protocol client for one page. */
async function connect(debugPort) {
  let targets;
  await waitFor("Chrome's DevTools endpoint", async () => {
    targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
    return targets.some((t) => t.type === "page");
  });
  const ws = new WebSocket(targets.find((t) => t.type === "page").webSocketDebuggerUrl);
  await new Promise((res, rej) => {
    ws.onopen = res;
    ws.onerror = rej;
  });
  let id = 0;
  const pending = new Map();
  const errors = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) {
      pending.get(d.id)(d);
      pending.delete(d.id);
    }
    if (d.method === "Runtime.exceptionThrown") {
      errors.push(d.params.exceptionDetails.exception?.description ?? d.params.exceptionDetails.text);
    }
    if (d.method === "Runtime.consoleAPICalled" && d.params.type === "error") {
      errors.push(d.params.args.map((a) => a.value ?? a.description).join(" "));
    }
  };
  const send = (method, params = {}) =>
    new Promise((res, rej) => {
      const i = ++id;
      pending.set(i, (d) => (d.error ? rej(new Error(`${method}: ${d.error.message}`)) : res(d.result)));
      ws.send(JSON.stringify({ id: i, method, params }));
    });
  const evaluate = async (expression) =>
    (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result
      ?.value;
  await send("Runtime.enable");
  await send("DOM.enable");
  return { send, evaluate, errors, close: () => ws.close() };
}

/** Load the app for one shot and bring it to the state the shot shows. */
async function stage(page, url, shot) {
  const d = DEVICES[shot.device];
  await page.send("Emulation.setDeviceMetricsOverride", {
    width: d.width,
    height: d.height,
    deviceScaleFactor: 2,
    mobile: d.mobile,
  });
  await page.send("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-color-scheme", value: shot.theme }],
  });
  await page.send("Page.navigate", { url });
  const ready = () =>
    page.evaluate(
      `!!document.querySelector('[data-testid=cone]') && !document.querySelector('.dot.busy')`,
    );
  await waitFor("the demo forecast", ready);

  if (shot.source === "sandbox") {
    const { root } = await page.send("DOM.getDocument");
    const { nodeId } = await page.send("DOM.querySelector", {
      nodeId: root.nodeId,
      selector: "[data-testid=file-input]",
    });
    await page.send("DOM.setFileInputFiles", { nodeId, files: [SANDBOX_FEED] });
    await waitFor(
      "the sandbox forecast",
      async () =>
        (await page.evaluate(
          `document.querySelector('[data-testid=source]').textContent.includes('file')`,
        )) && (await ready()),
    );
  }

  // An afford answer, so that section is not empty.
  await page.evaluate(`document.querySelector('.afford button[type=submit]').click()`);
  await waitFor("the afford answer", () =>
    page.evaluate(`!!document.querySelector('[data-testid=afford-answer]')`),
  );

  if (shot.view === "lows") {
    await page.evaluate(
      `[...document.querySelectorAll('.segmented button')].find((b) => b.textContent.includes('lows')).click()`,
    );
    await waitFor("the lows view", () =>
      page.evaluate(`document.querySelector('[data-testid=cone]').dataset.view === 'lows'`),
    );
  }

  if (await page.evaluate(`document.documentElement.scrollWidth > document.documentElement.clientWidth`)) {
    throw new Error(`${name(shot)}: the page scrolls sideways`);
  }
  // Let layout (the chart measures its box) and fonts settle.
  await sleep(300);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.build || !existsSync(join(WEB, "dist/index.html"))) {
    console.log("building web/dist …");
    const b = spawnSync("npm", ["run", "build"], { cwd: WEB, stdio: "inherit" });
    if (b.status !== 0) throw new Error("npm run build failed");
  }
  let shots = args.all ? allShots() : README_SET;
  if (!existsSync(SANDBOX_FEED)) {
    console.warn(`no ${SANDBOX_FEED}: skipping the sandbox shots`);
    shots = shots.filter((s) => s.source !== "sandbox");
  }
  mkdirSync(args.out, { recursive: true });

  const webPort = await freePort();
  const debugPort = await freePort();
  const url = `http://127.0.0.1:${webPort}/`;
  const server = spawn(
    join(WEB, "node_modules/.bin/vite"),
    ["preview", "--host", "127.0.0.1", "--port", String(webPort), "--strictPort"],
    { cwd: WEB, stdio: "ignore" },
  );
  const profile = mkdtempSync(join(tmpdir(), "ephemeris-shots-"));
  const chrome = spawn(
    findChrome(),
    [
      "--headless=new",
      `--remote-debugging-port=${debugPort}`,
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--hide-scrollbars",
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  let page;
  try {
    await waitFor("vite preview", async () => (await fetch(url)).ok);
    page = await connect(debugPort);
    for (const shot of shots) {
      await stage(page, url, shot);
      const { data } = await page.send("Page.captureScreenshot", { format: "png" });
      const path = join(args.out, name(shot));
      writeFileSync(path, Buffer.from(data, "base64"));
      console.log(path);
    }
    if (page.errors.length > 0) {
      throw new Error(`page errors:\n${page.errors.join("\n")}`);
    }
  } finally {
    page?.close();
    await Promise.all([stop(chrome), stop(server)]);
    // Chrome may still be flushing its profile; a leftover temp dir is harmless.
    try {
      rmSync(profile, { recursive: true, force: true });
    } catch {
      console.warn(`could not remove ${profile}`);
    }
  }
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
