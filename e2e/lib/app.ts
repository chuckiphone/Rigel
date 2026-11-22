import { browser, $ } from "@wdio/globals";

/** The window's whole visible text, which is how these specs assert on state. */
export const bodyText = async () => (await $("body")).getText();

/**
 * Click by visible text in one DOM step.
 *
 * The rail swaps the whole surface, so a handle found a moment earlier can be unmounted before
 * the click lands. Querying and clicking in the same script is stable, and the click is still a
 * real click on the real button, so React handles it exactly as it would a mouse click.
 */
async function clickByText(needle: string): Promise<boolean> {
  return (await browser.execute((text: string) => {
    const button = Array.from(document.querySelectorAll("button")).find((candidate) =>
      (candidate.textContent ?? "").includes(text),
    );
    if (!button) return false;
    button.click();
    return true;
  }, needle)) as boolean;
}

export async function openRail(label: string) {
  const rail = await $('nav[aria-label="Primary"]');
  await rail.waitForDisplayed({ timeout: 40_000 });
  const opened = (await browser.execute((name: string) => {
    const button = document.querySelector<HTMLButtonElement>(
      `nav[aria-label="Primary"] button[aria-label="${name}"]`,
    );
    if (!button) return false;
    button.click();
    return true;
  }, label)) as boolean;
  if (!opened) throw new Error(`the rail has no ${label} button`);
}

/** Account and Settings live outside the rail now, in the fixed bottom-right utility cluster. */
export async function openUtility(label: string) {
  const cluster = await $('[role="group"][aria-label="Account and settings"]');
  await cluster.waitForDisplayed({ timeout: 40_000 });
  const opened = (await browser.execute((name: string) => {
    const button = document.querySelector<HTMLButtonElement>(
      `[role="group"][aria-label="Account and settings"] button[aria-label="${name}"]`,
    );
    if (!button) return false;
    button.click();
    return true;
  }, label)) as boolean;
  if (!opened) throw new Error(`the utility cluster has no ${label} button`);
}

/** Open Settings and one page in its left column. The row is a real button with React state. */
export async function openSettingsPage(label: string) {
  await openUtility("Settings");
  const found = await browser.waitUntil(
    async () =>
      (await browser.execute((name: string) => {
        const rows = Array.from(
          document.querySelectorAll<HTMLButtonElement>(".settings-nav button.row"),
        );
        return rows.some((row) => (row.textContent ?? "").includes(name));
      }, label)) === true,
    { timeout: 30_000, interval: 100, timeoutMsg: `Settings has no ${label} page` },
  );
  if (!found) throw new Error(`Settings has no ${label} page`);
  const opened = await browser.execute((name: string) => {
    const rows = Array.from(
      document.querySelectorAll<HTMLButtonElement>(".settings-nav button.row"),
    );
    const row = rows.find((candidate) => (candidate.textContent ?? "").includes(name));
    row?.click();
    return Boolean(row);
  }, label);
  if (!opened) throw new Error(`Settings has no ${label} page`);
}

/**
 * Start a session from whichever affordance the surface shows: a project with no session offers
 * "Start a session", one that already has sessions offers the "+ session" button.
 */
export async function startSession() {
  // The project must be open before the Session surface can offer a start control. This is the
  // same readiness condition the screenshot and window specs assert explicitly.
  await waitForText("e2e-project");
  await openRail("Session");
  // The rail swaps the surface asynchronously. A fresh app can show the nav before the
  // Session surface has rendered its start control; wait for the control instead of racing it.
  await browser.waitUntil(
    async () =>
      (await browser.execute(() => {
        const labels = ["Start a session", "+ session"];
        return Array.from(document.querySelectorAll("button")).some((candidate) =>
          labels.some((label) => (candidate.textContent ?? "").includes(label)),
        );
      })) === true,
    {
      timeout: 30_000,
      interval: 100,
      timeoutMsg: "the session surface never offered a start control",
    },
  );
  if (!(await clickByText("Start a session")) && !(await clickByText("+ session"))) {
    throw new Error("the session surface offered no way to start a session");
  }
  const composer = await $('textarea[placeholder*="Ask"]');
  await composer.waitForDisplayed({ timeout: 30_000 });
}

export async function sendPrompt(text: string) {
  const composer = await $('.composer-field textarea');
  await composer.waitForDisplayed({ timeout: 20_000 });
  await composer.setValue(text);
  const send = await $('button[aria-label="Send"]');
  await send.waitForDisplayed({ timeout: 20_000 });
  await send.click();
}

/** Type a follow-up while a turn runs; the arrow is the queue action in that state. */
export async function queuePrompt(text: string) {
  const composer = await $('.composer-field textarea');
  await composer.waitForDisplayed({ timeout: 20_000 });
  await composer.setValue(text);
  const queue = await $('button[aria-label="Queue message"]');
  await queue.waitForDisplayed({ timeout: 20_000 });
  await queue.click();
}

/** Count a phrase in the visible window. Queue menus must be closed for this to mean transcript. */
export async function textCount(needle: string): Promise<number> {
  const text = await bodyText();
  return text.split(needle).length - 1;
}

/** The approval card, whether or not the current turn asked anything. */
export async function approvalCard() {
  return $('section[aria-label="Approval required"]');
}

export async function waitForText(needle: string, timeout = 40_000) {
  await browser.waitUntil(async () => (await bodyText()).includes(needle), {
    timeout,
    timeoutMsg: `the window never showed ${JSON.stringify(needle)}`,
  });
}

/**
 * Approve every card the turn raises: a turn that runs a command and then writes a file asks
 * once for each, and each answer resumes the same turn.
 *
 * `doneAtLeast` is the transcript's count of the scripted finish line. A second turn in the same
 * transcript passes `2`, so the first turn's `Done.` cannot make this helper stop before the new
 * turn has actually finished.
 */
export async function approveEveryCard(limit = 4, doneAtLeast = 1) {
  for (let seen = 0; seen < limit; seen += 1) {
    const card = await approvalCard();
    // `sendPrompt` returns before the coordinator has necessarily raised its first card. Wait for
    // either the next card or the end of the turn; checking once races a fast scripted reply.
    let raised = false;
    await browser.waitUntil(
      async () => {
        if (await card.isExisting()) {
          raised = true;
          return true;
        }
        return (await textCount("Done.")) >= doneAtLeast;
      },
      { timeout: 30_000, interval: 150, timeoutMsg: "the turn raised no card and did not finish" },
    );
    if (!raised) return seen;
    await card.waitForDisplayed({ timeout: 20_000 });
    await (await card.$("button=Allow once")).click();
    await browser.pause(400);
  }
  return limit;
}
