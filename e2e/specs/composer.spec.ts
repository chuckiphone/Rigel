import { $, $$, browser, expect } from "@wdio/globals";
import {
  approvalCard,
  bodyText,
  openRail,
  queuePrompt,
  sendPrompt,
  startSession,
  textCount,
} from "../lib/app";

/**
 * The real Tauri window, the real coordinator and the real approval channel. The model is the
 * deterministic ScriptedModel, not a vendor call; every turn below runs through `turn_send`, the
 * native loop, tools, persistence and `TurnFinished`.
 */

const QUEUED = "Queued follow-up one.";
const HELD = "Held after stop.";

async function waitForCount(needle: string, count: number, timeout = 60_000) {
  await browser.waitUntil(
    async () => (await textCount(needle)) === count,
    {
      timeout,
      interval: 200,
      timeoutMsg: `expected ${count} visible ${JSON.stringify(needle)} rows, still saw ${await textCount(needle)}`,
    },
  );
}

async function approveOne() {
  const card = await approvalCard();
  await card.waitForDisplayed({ timeout: 30_000 });
  await (await card.$("button=Allow once")).click();
  await browser.waitUntil(async () => !(await card.isExisting()), {
    timeout: 15_000,
    interval: 150,
    timeoutMsg: "the approval card did not close after Allow once",
  });
}

async function stopTurn() {
  const stop = await $('button[aria-label="Stop turn"]');
  await stop.waitForDisplayed({ timeout: 20_000 });
  await expect(stop).toBeEnabled();
  await stop.click();
  await browser.waitUntil(async () => !(await stop.isEnabled()), {
    timeout: 20_000,
    interval: 150,
    timeoutMsg: "the stop control stayed enabled after Stop",
  });
}

/**
 * Wait for one scripted turn to close, answering whatever card it raises on the way.
 *
 * The queue test only needs the turn to finish, but a turn can raise its command card a moment
 * after its prompt row paints, and a slow runner can deliver the final state late. A card whose
 * answer did not land is published again by the polling fallback, so one optimistic click is
 * not proof the turn resumed; keep answering until Stop disables. A timeout records the visible
 * state so the next failure names what the window was showing.
 */
async function finishTurn(label: string, timeout = 45_000) {
  const deadline = Date.now() + timeout;
  let approved = 0;
  while (Date.now() < deadline) {
    const stop = await $('button[aria-label="Stop turn"]');
    if (!(await stop.isEnabled())) {
      if (approved === 0) {
        throw new Error(`${label}: the turn closed without asking for the scripted command`);
      }
      return;
    }
    const card = await approvalCard();
    if (approved < 4 && (await card.isExisting())) {
      const allow = await card.$("button=Allow once");
      if (await allow.isExisting()) {
        await allow.click();
        approved += 1;
        await browser.pause(250);
      }
    }
    await browser.pause(150);
  }
  const state = (await browser.execute(() => {
    const stop = document.querySelector<HTMLButtonElement>('button[aria-label="Stop turn"]');
    const queue = document.querySelector('button[aria-label^="Message queue"]');
    return {
      stopEnabled: stop ? !stop.disabled : null,
      cards: document.querySelectorAll('section[aria-label="Approval required"]').length,
      queue: queue?.getAttribute("aria-label") ?? null,
      tail: (document.body.textContent ?? "").slice(-1_200),
    };
  })) as unknown;
  throw new Error(`${label}: the turn did not finish; ${JSON.stringify(state)}`);
}

describe("the real composer queue", () => {
  it("queues against a running turn, drains once, and holds the line after Stop", async () => {
    await (await $('nav[aria-label="Primary"]')).waitForDisplayed({ timeout: 40_000 });
    await startSession();

    // Turn 1 is genuinely running while its first command waits for approval.
    await sendPrompt("Queue test turn one.");
    const firstCard = await approvalCard();
    await firstCard.waitForDisplayed({ timeout: 30_000 });

    await queuePrompt(QUEUED);
    const queueButton = await $('button[aria-label^="Message queue"]');
    await expect(queueButton).toHaveAttribute("aria-label", "Message queue: 1 waiting");
    await queueButton.click();
    const queueMenu = await $('div[role="menu"][aria-label="Message queue"]');
    await queueMenu.waitForDisplayed({ timeout: 10_000 });
    await expect(queueMenu).toHaveText(expect.stringContaining(QUEUED));
    await queueButton.click();
    await expect(queueMenu).not.toBeExisting();
    await expect(await textCount(QUEUED)).toBe(0);

    await approveOne();

    // The queued prompt starts by itself after TurnFinished, exactly once.
    await waitForCount(QUEUED, 1);
    await finishTurn("the queued turn");

    // Turn 3 waits on approval while a follow-up is queued. Stop must close it and hold the line.
    await sendPrompt("Stop test turn.");
    const stopCard = await approvalCard();
    await stopCard.waitForDisplayed({ timeout: 30_000 });
    await queuePrompt(HELD);
    await expect(await textCount(HELD)).toBe(0);
    await stopTurn();
    await browser.waitUntil(async () => !(await stopCard.isExisting()), {
      timeout: 20_000,
      interval: 150,
      timeoutMsg: "the approval card survived Stop",
    });
    await expect(queueButton).toHaveAttribute("aria-label", "Message queue: 1 waiting");
    await expect(await textCount(HELD)).toBe(0);
    await browser.pause(1_500);
    await expect(await textCount(HELD)).toBe(0);
    const cancelled = await $$('article[aria-label$=", cancelled"]');
    await expect(cancelled.length).toBeGreaterThan(0);

    // Retry is the only thing that releases the held line.
    await queueButton.click();
    await queueMenu.waitForDisplayed({ timeout: 10_000 });
    await expect(queueMenu).toHaveText(expect.stringContaining(HELD));
    await (await queueMenu.$("button=Retry")).click();
    await queueButton.click();
    await waitForCount(HELD, 1);
    await finishTurn("the resumed queued turn");

    await openRail("Diff");
    await expect($("body")).toHaveText(expect.stringContaining("README.md"));
  });
});
