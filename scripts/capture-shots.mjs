#!/usr/bin/env node
/**
 * Capture the current build.
 *
 * The renderer runs in a browser preview with sample rows (`ui/src/mock.ts`), which is exactly what
 * the Playwright suite already drives — so these are photographs of the shipped renderer with its
 * sample data, not mockups drawn for a README. Every still is taken at a 1440x900 viewport with a
 * 2x device scale. The tour records the current surfaces in motion; ffmpeg turns the recordings into
 * the GIFs and MP4s the README and the site link to.
 *
 * Usage: node scripts/capture-shots.mjs
 *   expects a preview on http://127.0.0.1:4173, or set RIGEL_PREVIEW_URL.
 *   requires ffmpeg on PATH for the GIF/MP4 step.
 */
import { createRequire } from "node:module";
import { copyFileSync, existsSync, mkdirSync, readdirSync, renameSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const require = createRequire(new URL("../ui/package.json", import.meta.url));
const { chromium } = require("playwright");

const BASE = process.env.RIGEL_PREVIEW_URL ?? "http://127.0.0.1:4173";
const SHOTS = resolve("web/shots");
const README_SHOTS = resolve("screenshots");
const VIDDIR = resolve("tmp/capture-video");
const PREFIX = "2026-";

for (const dir of [SHOTS, README_SHOTS, VIDDIR]) mkdirSync(dir, { recursive: true });
rmSync(VIDDIR, { recursive: true, force: true });
mkdirSync(VIDDIR, { recursive: true });

const taken = [];
const missed = [];

async function still(page, name) {
  try {
    for (const dir of [SHOTS, README_SHOTS]) {
      await page.screenshot({ path: join(dir, `${PREFIX}${name}.png`) });
    }
    taken.push(name);
  } catch (error) {
    missed.push(`${name}: ${error.message}`);
  }
}

async function click(page, role, name, timeout = 6000) {
  await page.getByRole(role, { name, exact: false }).first().click({ timeout });
}

function ffmpeg(args) {
  const result = spawnSync("ffmpeg", args, { stdio: "ignore" });
  return result.status === 0;
}

function publish(video, name) {
  const gif = join(VIDDIR, `${name}.gif`);
  const mp4 = join(VIDDIR, `${name}.mp4`);
  const filters =
    "fps=8,scale=1000:-1:flags=lanczos,split[s0][s1];" +
    "[s0]palettegen=max_colors=96:stats_mode=diff[p];" +
    "[s1][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle";
  const gifOk = ffmpeg(["-y", "-i", video, "-vf", filters, "-loop", "0", gif]);
  const mp4Ok = ffmpeg([
    "-y",
    "-i",
    video,
    "-c:v",
    "libx264",
    "-pix_fmt",
    "yuv420p",
    "-crf",
    "23",
    "-movflags",
    "+faststart",
    "-an",
    mp4,
  ]);
  if (gifOk) {
    for (const dir of [SHOTS, README_SHOTS]) {
      copyFileSync(gif, join(dir, `${name}.gif`));
    }
  }
  if (mp4Ok) {
    for (const dir of [SHOTS, README_SHOTS]) {
      copyFileSync(mp4, join(dir, `${name}.mp4`));
    }
  }
  return { gif: gifOk, mp4: mp4Ok };
}

const browser = await chromium.launch();

/* ------------------------------- stills ------------------------------- */
try {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    reducedMotion: "no-preference",
  });
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  await page.goto(BASE, { waitUntil: "networkidle" });
  await page.waitForTimeout(900);

  await still(page, "paper");

  await click(page, "button", "Shelf");
  await page.waitForTimeout(700);
  await still(page, "shelf");

  // The inspector is the point of the shelf: one row open beside the list.
  try {
    await page.getByRole("option", { name: /pdf/i }).first().click();
    await page.waitForTimeout(700);
    await still(page, "shelf-detail");
  } catch (error) {
    missed.push(`shelf-detail: ${error.message}`);
  }

  await click(page, "button", "Session");
  await page.waitForTimeout(800);
  await still(page, "session");

  await click(page, "button", "Chat");
  await page.waitForTimeout(700);
  await still(page, "chat");

  await click(page, "button", "Diff");
  await page.waitForTimeout(700);
  await still(page, "diff");

  await click(page, "button", "Settings");
  await page.waitForTimeout(600);
  try {
    await page.getByRole("option", { name: /Appearance/i }).first().click();
    await page.waitForTimeout(500);
  } catch (error) {
    missed.push(`settings-appearance: ${error.message}`);
  }
  await still(page, "settings");

  await click(page, "button", "Attention");
  await page.waitForTimeout(600);
  await still(page, "attention");

  await click(page, "button", "Shelf");
  await page.waitForTimeout(400);
  try {
    await page.keyboard.press("Control+k");
    await page.waitForTimeout(400);
    await page.keyboard.type("session", { delay: 40 });
    await page.waitForTimeout(400);
    await still(page, "palette");
    await page.keyboard.press("Escape");
  } catch (error) {
    missed.push(`palette: ${error.message}`);
  }

  await context.close();
} catch (error) {
  missed.push(`stills: ${error.message}`);
}

/* -------------------------------- tour -------------------------------- */
async function record(name, drive) {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    recordVideo: { dir: VIDDIR, size: { width: 1440, height: 900 } },
    reducedMotion: "no-preference",
  });
  const page = await context.newPage();
  page.setDefaultTimeout(8000);
  try {
    await page.goto(BASE, { waitUntil: "networkidle" });
    await page.waitForTimeout(1600);
    await drive(page);
    const path = await page.video().path();
    await context.close();
    const video = join(VIDDIR, `${name}.webm`);
    renameSync(path, video);
    return video;
  } catch (error) {
    missed.push(`${name}: ${error.message}`);
    await context.close().catch(() => {});
    return null;
  }
}

const tourVideo = await record("tour", async (page) => {
  // The paper: let the current edition settle, then read down the front page.
  await page.mouse.move(900, 600);
  await page.mouse.wheel(0, 360);
  await page.waitForTimeout(1800);
  await page.mouse.wheel(0, -360);

  // Shelf: open one row so the inspector animates in.
  await click(page, "button", "Shelf").catch(() => {});
  await page.waitForTimeout(700);
  await page.getByRole("option", { name: /pdf/i }).first().click().catch(() => {});
  await page.waitForTimeout(2200);

  // Session: scroll the transcript like a reader.
  await click(page, "button", "Session").catch(() => {});
  await page.waitForTimeout(900);
  await page.mouse.move(900, 600);
  await page.mouse.wheel(0, 420);
  await page.waitForTimeout(1600);
  await page.mouse.wheel(0, 420);
  await page.waitForTimeout(1400);

  await click(page, "button", "Chat").catch(() => {});
  await page.waitForTimeout(2200);

  await click(page, "button", "Diff").catch(() => {});
  await page.waitForTimeout(2400);

  // Settings: show the grouped pages and fold the notes rail.
  await click(page, "button", "Settings").catch(() => {});
  await page.waitForTimeout(800);
  await page.getByRole("option", { name: /Appearance/i }).first().click().catch(() => {});
  await page.waitForTimeout(1400);
  await page.getByRole("button", { name: /Fold notes/i }).first().click().catch(() => {});
  await page.waitForTimeout(1600);
  await page.getByRole("button", { name: /^Notes$/i }).first().click().catch(() => {});
  await page.waitForTimeout(900);

  // Palette: the keyboard path through the app.
  await page.keyboard.press("Control+k");
  await page.waitForTimeout(700);
  await page.keyboard.type("session", { delay: 55 });
  await page.waitForTimeout(1200);
  await page.keyboard.press("Escape");
  await page.waitForTimeout(800);
});

const demoVideo = await record("demo", async (page) => {
  await page.waitForTimeout(1300);
  await click(page, "button", "Session").catch(() => {});
  await page.waitForTimeout(1200);
  await page.mouse.move(900, 600);
  await page.mouse.wheel(0, 360);
  await page.waitForTimeout(1600);
  await click(page, "button", "Settings").catch(() => {});
  await page.waitForTimeout(900);
  await page.getByRole("option", { name: /Appearance/i }).first().click().catch(() => {});
  await page.waitForTimeout(1500);
});

await browser.close();

const made = [];
for (const [name, video] of [
  ["rigel-tour-2026", tourVideo],
  ["rigel-demo", demoVideo],
]) {
  if (!video || !existsSync(video)) {
    missed.push(`${name}: no recording`);
    continue;
  }
  const result = publish(video, name);
  if (!result.gif) missed.push(`${name}.gif: ffmpeg failed`);
  if (!result.mp4) missed.push(`${name}.mp4: ffmpeg failed`);
  made.push(`${name}.gif, ${name}.mp4`);
}

console.log(`still at ${BASE}`);
console.log(`  captured ${taken.length}: ${taken.join(", ")}`);
console.log(`  video: ${made.length ? made.join("; ") : "none"}`);
if (missed.length) {
  console.log("  skipped:");
  for (const line of missed) console.log(`    ${line}`);
}
const files = existsSync(SHOTS) ? readdirSync(SHOTS).filter((f) => f.startsWith(PREFIX)) : [];
console.log(`  files on disk: ${files.length}`);
if (missed.some((line) => line.endsWith("no recording"))) process.exitCode = 1;
