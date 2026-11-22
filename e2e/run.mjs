#!/usr/bin/env node
/**
 * Item 1: run the built Tauri app under tauri-driver + msedgedriver.
 *
 * `cargo build` sets `cfg(dev)`, so that binary points its webview at `devUrl` and shows an error
 * page when no Vite server is up. `tauri build --debug` embeds `ui/dist` instead; that is the
 * binary this runner drives.
 *
 * msedgedriver's own launch path no longer reaches the WebView2 debug port on this runtime
 * (`DevToolsActivePort file doesn't exist`), so the app is started here with a fixed
 * `--remote-debugging-port` and msedgedriver attaches to it through `ms:edgeOptions`. tauri-driver
 * stays in the middle: it is the WebDriver endpoint the specs talk to.
 *
 * The turn itself needs no provider: `RIGEL_E2E_SCRIPT` makes `turn_send` use the core's
 * ScriptedModel, while approvals, tools, patches, the transcript and the bill stay on the
 * production path. The window's key gate still asks whether any chat provider is configured, so
 * the harness seeds a deliberately fake OpenRouter key; its base URL points at a closed loopback
 * port, and the scripted model means no request reaches a vendor.
 *
 * Usage: node e2e/run.mjs [--skip-build] [--spec <file>]
 *
 * With no --spec, each spec gets its own app process. The window specs deliberately drive one
 * project, but a session and its render state must not leak into the next spec; a fresh process
 * also means a failed page cannot make the following spec look like a product failure.
 */

import { execFileSync, spawn } from "node:child_process";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
// A run-scoped directory: a killed WebView2 can keep files locked, and a wipe that fails would
// fail the suite for reasons that have nothing to do with the app.
const STAMP = new Date().toISOString().replace(/[:.]/g, "-");
const WORK = path.join(ROOT, "tmp", "e2e", `run-${STAMP}`);
const PROJECT = path.join(WORK, "e2e-project");
const APP = path.join(ROOT, "target", "debug", "rigel-app.exe");
const DRIVER = path.join(process.env.USERPROFILE ?? "", ".cargo", "bin", "tauri-driver.exe");
const PORT = 4446;
const NATIVE_PORT = 9516;
// The Gravity spec uses the local harness broker. Keep its URL in one place so the app and the
// spec agree even when the caller has not exported either variable.
let GRAVITY_BROKER = (
  process.env.RIGEL_GRAVITY_BROKER_URL ?? "http://127.0.0.1:5000"
).replace(/\/+$/, "");
let GRAVITY_ENDPOINT =
  process.env.RIGEL_GRAVITY_ENDPOINT ?? `${GRAVITY_BROKER}/api/gravity-ad`;
const GRAVITY_CONFIGURED = Boolean(
  process.env.RIGEL_GRAVITY_BROKER_URL || process.env.RIGEL_GRAVITY_ENDPOINT,
);
// A fixture fill for the cold path: no publisher key, no Functions process, and no network call
// to Gravity's ad engine. The tracking URLs still point at Gravity's trusted host, so the Rust
// impression receipt and click-through remain real rather than mocked in the renderer.
const GRAVITY_FIXTURE_CACHED = {
  title: "Run coding agents on your existing AI subscriptions",
  brand: "Openllm",
  text: "Run coding agents and clients on your existing AI subscriptions with OpenLLM's developer-focused gateway.",
  cta: "Get Started",
  click: "https://trygravity.ai",
  impression: "https://trygravity.ai",
  favicon: "https://github.com/neondatabase.png?size=200",
};
const GRAVITY_FIXTURE_AD = {
  adText: GRAVITY_FIXTURE_CACHED.text,
  title: GRAVITY_FIXTURE_CACHED.title,
  brandName: GRAVITY_FIXTURE_CACHED.brand,
  cta: GRAVITY_FIXTURE_CACHED.cta,
  url: "https://openllm.sh",
  favicon: GRAVITY_FIXTURE_CACHED.favicon,
  clickUrl: GRAVITY_FIXTURE_CACHED.click,
  impUrl: GRAVITY_FIXTURE_CACHED.impression,
  placement: "below_response",
  placement_id: "rigel-sponsor",
};
// Chosen from the OS's free-port pool in main(). A PID-derived port in a narrow range can land in
// a Windows excluded range on a hosted runner, leaving WebView2 unable to bind its debug socket.
let CDP_PORT = 9300 + (process.pid % 300);

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const specIndex = args.indexOf("--spec");
const spec = specIndex >= 0 ? args[specIndex + 1] : null;

function run(command, commandArgs, options = {}) {
  console.log(`\n$ ${command} ${commandArgs.join(" ")}`);
  execFileSync(command, commandArgs, { cwd: ROOT, stdio: "inherit", ...options });
}

/** Ask the OS for a currently free loopback port, then release it for WebView2. */
function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : 0;
      server.close((error) => (error ? reject(error) : resolve(port)));
    });
  });
}

/** Run one spec in its own process while retaining its output for a hosted annotation. */
function runSpec(name) {
  return new Promise((resolve) => {
    const child = spawn(
      process.execPath,
      [fileURLToPath(import.meta.url), "--skip-build", "--spec", `e2e/specs/${name}`],
      { cwd: ROOT, env: { ...process.env }, stdio: ["ignore", "pipe", "pipe"] },
    );
    let output = "";
    const collect = (chunk, stream) => {
      const text = chunk.toString();
      output += text;
      stream.write(text);
    };
    child.stdout.on("data", (chunk) => collect(chunk, process.stdout));
    child.stderr.on("data", (chunk) => collect(chunk, process.stderr));
    child.on("error", (error) => resolve({ code: 1, output: `${output}\n${error}` }));
    child.on("exit", (code) => resolve({ code: code ?? 1, output }));
  });
}

/**
 * The specs live beside this file but the WebdriverIO packages are installed in `ui/`. Node
 * resolves `@wdio/globals` upward from the spec, so the nearest `node_modules` has to be this one.
 */
function ensureModuleLink() {
  const link = path.join(ROOT, "e2e", "node_modules");
  if (existsSync(link)) return;
  symlinkSync(
    path.join(ROOT, "ui", "node_modules"),
    link,
    process.platform === "win32" ? "junction" : "dir",
  );
}

/** Old run directories can hold WebView2 locks; best effort, never fatal. */
function pruneRuns(keepMs = 12 * 60 * 60 * 1000) {
  const root = path.join(ROOT, "tmp", "e2e");
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (!entry.isDirectory() || !entry.name.startsWith("run-")) continue;
    const full = path.join(root, entry.name);
    if (full === WORK) continue;
    try {
      if (Date.now() - statSync(full).mtimeMs < keepMs) continue;
      rmSync(full, { recursive: true, force: true, maxRetries: 2 });
    } catch {
      // A live WebView2 process still owns it; a later run will try again.
    }
  }
}

function prepareWorkspace() {
  mkdirSync(WORK, { recursive: true });
  pruneRuns();
  for (const dir of ["appdata", "localappdata", "index", "webview"]) {
    mkdirSync(path.join(WORK, dir), { recursive: true });
  }
  // The window's Home surface reads settings.toml from APPDATA. Seed the two Gravity switches
  // here so the real window test proves the spawned card without driving the Settings UI first.
  const appData = path.join(WORK, "appdata", "Rigel");
  mkdirSync(appData, { recursive: true });
  // Route spec: Groq is a real non-gateway door with no model namespace of its own, so it accepts
  // the saved llama id (`model-only`) but refuses a bare DeepSeek id, which then repairs to the
  // door that owns it. Other specs keep the default pair.
  const savedPair = spec?.endsWith("route.spec.ts")
    ? ['provider = "groq"', 'model = "llama-4"']
    : [];
  writeFileSync(
    path.join(appData, "settings.toml"),
    [
      'theme = "dark"',
      "default_budget_cents = 500",
      "gravity_ads = true",
      "gravity_test_ads = true",
      ...savedPair,
      "",
    ].join(String.fromCharCode(10)),
  );
  mkdirSync(PROJECT, { recursive: true });
  writeFileSync(path.join(PROJECT, "README.md"), "# E2E\n");
  writeFileSync(path.join(PROJECT, "note.txt"), "unchanged\n");

  // Three steps: ask for a destructive command, edit a file, close. A reply with no tool calls
  // ends the turn, so no announce text can come first. The approval card and the diff come from
  // the real coordinator and the real patch pipeline, not from a fixture.
  const changed = ["# E2E", "", "Approved change.", ""].join(String.fromCharCode(10));
  const script = {
    steps: [
      {
        tool_calls: [
          {
            name: "run_command",
            arguments: { command: "rm -rf $RIGEL_E2E_MISSING_DIR" },
          },
        ],
      },
      {
        tool_calls: [
          {
            name: "read_file",
            arguments: { path: "README.md" },
          },
        ],
      },
      {
        tool_calls: [
          {
            name: "edit_file",
            arguments: {
              path: "README.md",
              old_string: "# E2E",
              new_string: changed,
            },
          },
        ],
      },
      {
        text: "Done.",
        usage: { input_tokens: 30, output_tokens: 5, cost_usd: 0.0001 },
      },
    ],
  };
  writeFileSync(path.join(WORK, "script.json"), JSON.stringify(script, null, 2));
}

async function waitForCdp(timeoutMs = 40_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`);
      if (response.ok) {
        const body = await response.json();
        console.log(`webview: ${body.Browser}`);
        return;
      }
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`the app never opened a debug port on ${CDP_PORT}`);
}

async function waitForDriver(timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${PORT}/status`);
      if (response.ok) {
        const body = await response.json();
        if (body?.value?.ready) {
          console.log(`driver: ${body.value.message}`);
          return;
        }
      }
    } catch {
      // Not listening yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`tauri-driver did not become ready on port ${PORT}`);
}

/** Fail before launching WebView2 when an explicitly configured broker is not answering. */
async function waitForGravityBroker(broker = GRAVITY_BROKER, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  let last = "not reachable";
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${broker}/api/test-state`, { cache: "no-store" });
      if (response.ok) {
        const body = await response.json();
        console.log(`gravity broker: ${broker} (fill=${body.fill})`);
        return;
      }
      last = `HTTP ${response.status}`;
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(
    `the Gravity spec needs the configured local broker at ${broker}/api/test-state (${last})`,
  );
}

/** One quick look, so a live broker is reused rather than shadowed by the fixture. */
async function gravityBrokerState(broker) {
  try {
    const response = await fetch(`${broker}/api/test-state`, {
      cache: "no-store",
      signal: AbortSignal.timeout(1_500),
    });
    if (!response.ok) return null;
    const body = await response.json();
    return body && typeof body.fill === "boolean" ? body : null;
  } catch {
    return null;
  }
}

/** The broker replacement for a cold machine: a real remote mark, served on a free loopback port. */
function startGravityFixture() {
  return new Promise((resolve, reject) => {
    let fill = process.env.RIGEL_GRAVITY_EXPECT_NO_FILL !== "1";
    let noArt = process.env.RIGEL_GRAVITY_NO_ART === "1";
    const server = createHttpServer((request, response) => {
      const chunks = [];
      request.on("data", (chunk) => chunks.push(chunk));
      request.on("end", () => {
        const url = new URL(request.url || "/", "http://127.0.0.1");
        const send = (status, body) => {
          response.writeHead(status, {
            "cache-control": "no-store",
            "content-type": "application/json; charset=utf-8",
          });
          response.end(JSON.stringify(body));
        };
        if (url.pathname === "/api/test-state") {
          send(200, { fill, cached: fill ? GRAVITY_FIXTURE_CACHED : null, noArt });
          return;
        }
        if (url.pathname === "/api/test-mode" && request.method === "POST") {
          try {
            const next = JSON.parse(Buffer.concat(chunks).toString("utf8"));
            if (typeof next.fill === "boolean") fill = next.fill;
            if (typeof next.noArt === "boolean") noArt = next.noArt;
          } catch {
            // Keep the current mode.
          }
          send(200, { fill, noArt });
          return;
        }
        if (url.pathname === "/api/gravity-ad") {
          if (request.method !== "POST") {
            send(405, { error: "method_not_allowed" });
            return;
          }
          const ad = noArt
            ? {
                ...GRAVITY_FIXTURE_AD,
                favicon: "http://cdn.example/northstar.png",
                brandImage: "",
                brandName: "Northstar Analytics",
              }
            : GRAVITY_FIXTURE_AD;
          send(200, { ads: fill ? [ad] : [], test: true });
          return;
        }
        send(404, { error: "not_found" });
      });
    });
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : 0;
      resolve({ server, url: `http://127.0.0.1:${port}` });
    });
  });
}

function stopTree(child) {
  if (!child || child.exitCode !== null) return;
  if (process.platform === "win32") {
    try {
      execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
      return;
    } catch {
      // Fall through to the portable kill.
    }
  }
  child.kill("SIGTERM");
}

async function main() {
  ensureModuleLink();
  let gravityFixture = null;
  if (!skipBuild) {
    // The Tauri CLI parses `CI`; `CI=1` is valid for most tools but not for it.
    run("cargo", ["tauri", "build", "--debug", "--no-bundle"], {
      cwd: path.join(ROOT, "crates", "rigel-app"),
      env: { ...process.env, CI: "true" },
    });
  }
  if (!existsSync(APP)) throw new Error(`missing app binary: ${APP}`);
  if (spec?.endsWith("gravity.spec.ts")) {
    if (GRAVITY_CONFIGURED) {
      await waitForGravityBroker();
    } else {
      const existing = await gravityBrokerState(GRAVITY_BROKER);
      if (existing) {
        console.log(`gravity broker: ${GRAVITY_BROKER} (existing, fill=${existing.fill})`);
      } else {
        const fixture = await startGravityFixture();
        gravityFixture = fixture.server;
        GRAVITY_BROKER = fixture.url;
        GRAVITY_ENDPOINT = `${fixture.url}/api/gravity-ad`;
        console.log(`gravity broker: ${GRAVITY_BROKER} (fixture)`);
      }
    }
  }

  if (!spec) {
    const specs = readdirSync(path.join(ROOT, "e2e", "specs"))
      .filter((name) => name.endsWith(".spec.ts"))
      .sort();
    if (specs.length === 0) throw new Error("no window specs found");
    const failureLog = path.join(ROOT, "tmp", "e2e", "window-failure.log");
    mkdirSync(path.dirname(failureLog), { recursive: true });
    let failed = 0;
    for (const name of specs) {
      let result = await runSpec(name);
      // A WebView2 process that still owns its profile can make the next fresh app fail before
      // the debug port binds. Retry that startup failure once; assertions are never retried.
      if (
        result.code !== 0 &&
        /never opened a debug port|app exited early|tauri-driver did not become ready/.test(
          result.output,
        )
      ) {
        console.error(`window spec startup failed: ${name}; retrying once`);
        await new Promise((resolve) => setTimeout(resolve, 2_000));
        result = await runSpec(name);
      }
      if (result.code === 0) {
        // Let the killed app and its WebView2 children release ports before the next spec.
        await new Promise((resolve) => setTimeout(resolve, 1_000));
        continue;
      }
      failed += 1;
      const lines = result.output
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter(Boolean);
      const detail =
        lines.find((line) => /^Error:|^AssertionError:|FAILED in |✖ /.test(line)) ??
        lines.slice(-3).join(" | ") ??
        "no diagnostic output";
      const message = `${name}: ${detail}`.slice(0, 900).replace(/\r?\n/g, " ");
      console.error(`window spec failed: ${message}`);
      console.log(`::error title=Window spec failed::${message}`);
      writeFileSync(failureLog, `${message}\n\n${result.output.slice(-12000)}\n`);
    }
    process.exit(failed === 0 ? 0 : 1);
  }
  if (!existsSync(DRIVER)) throw new Error(`missing tauri-driver: ${DRIVER}`);
  CDP_PORT = await freePort();
  prepareWorkspace();

  const env = {
    ...process.env,
    RIGEL_E2E_PROJECT: PROJECT,
    RIGEL_E2E_SCRIPT: path.join(WORK, "script.json"),
    RIGEL_INDEX_DIR: path.join(WORK, "index"),
    RIGEL_GRAVITY_ENDPOINT: GRAVITY_ENDPOINT,
    RIGEL_GRAVITY_BROKER_URL: GRAVITY_BROKER,
    // The Gravity spec clicks a sponsored card; record the click without opening the developer's
    // real browser window and tab.
    RIGEL_E2E_SUPPRESS_EXTERNAL_BROWSER: "1",
    APPDATA: path.join(WORK, "appdata"),
    LOCALAPPDATA: path.join(WORK, "localappdata"),
    // USERPROFILE and HOME stay real: redirecting them stops WebView2 from opening the debug
    // port at all (measured: arg reaches the browser, nothing binds). Settings and the index are
    // already isolated by APPDATA, LOCALAPPDATA, RIGEL_INDEX_DIR and the webview folder.
    RIGEL_OMNIROUTE_URL: "http://127.0.0.1:9",
    // The window's key gate needs one configured chat provider. This is not a real credential:
    // the scripted model replaces the network door, and this base URL refuses roster reads.
    OPENROUTER_API_KEY: "e2e-not-a-real-key",
    RIGEL_OPENROUTER_BASE_URL: "http://127.0.0.1:9",
    // A fixed port, so msedgedriver can attach to a webview this runner started.
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
    // WebView2 honours the debug port only while it starts a browser process. A folder that a
    // still-running process owns is joined without it, so every run gets its own folder.
    WEBVIEW2_USER_DATA_FOLDER: path.join(WORK, "webview"),
  };

  const app = spawn(APP, [], {
    cwd: ROOT,
    env,
    stdio: ["ignore", "pipe", "pipe"],
    // A debug build is a console binary; without this Windows opens a terminal for the app (and
    // its control API logs) on every run. The pipes still carry stdout/stderr to this log.
    windowsHide: true,
  });
  app.stdout.on("data", (chunk) => process.stdout.write(`app: ${chunk}`));
  app.stderr.on("data", (chunk) => process.stderr.write(`app: ${chunk}`));
  app.on("exit", (code) => {
    if (code !== 0) console.error(`app exited early with ${code}`);
  });

  let driver = null;
  let exitCode = 1;
  try {
    await waitForCdp();
    driver = spawn(DRIVER, ["--port", String(PORT), "--native-port", String(NATIVE_PORT)], {
      cwd: ROOT,
      env,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    driver.stdout.on("data", (chunk) => process.stdout.write(`driver: ${chunk}`));
    driver.stderr.on("data", (chunk) => process.stderr.write(`driver: ${chunk}`));
    await waitForDriver();

    const wdio = path.join(ROOT, "ui", "node_modules", "@wdio", "cli", "bin", "wdio.js");
    const wdioArgs = ["run", "e2e/wdio.conf.ts"];
    if (spec) wdioArgs.push("--spec", spec);
    console.log(`\n$ node ${wdioArgs.join(" ")}`);
    exitCode = await new Promise((resolve) => {
      const child = spawn(process.execPath, [wdio, ...wdioArgs], {
        cwd: ROOT,
        env: { ...env, RIGEL_E2E_CDP_PORT: String(CDP_PORT), RIGEL_E2E_PORT: String(PORT) },
        stdio: "inherit",
      });
      child.on("exit", (code) => resolve(code ?? 1));
    });
  } finally {
    stopTree(driver);
    stopTree(app);
    if (gravityFixture) {
      await new Promise((resolve) => gravityFixture.close(() => resolve()));
    }
  }
  console.log(`\nwindow e2e exit: ${exitCode}`);
  process.exit(exitCode);
}

main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(error);
  console.log(`::error title=Window runner failed::${message.replace(/\r?\n/g, " ").slice(0, 900)}`);
  process.exit(1);
});
