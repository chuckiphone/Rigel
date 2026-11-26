import { expect, test } from "@playwright/test";

// D151: history is recorded when a turn is actually delivered, and a snippet is only ever an
// insertion — the review card still stands between the draft and the model.
test("composer history recalls a sent prompt and a snippet fills its placeholder", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const composer = page.getByPlaceholder(/Ask the agent/);
  await composer.fill("First prompt for history");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await page.getByRole("button", { name: "Send original" }).click();
  await expect(composer).toHaveValue("");

  await composer.click();
  await page.keyboard.press("ArrowUp");
  await expect(composer).toHaveValue("First prompt for history");

  await composer.fill("");
  await page.getByRole("button", { name: "Add context" }).click();
  await page.getByRole("menuitem", { name: /Review the diff/ }).click();
  await expect(composer).toHaveValue(/Review the current diff for \{risk\}/);

  // Inserting selects the first placeholder, so typing replaces it rather than appending.
  await page.keyboard.type("the rollback path");
  await expect(composer).toHaveValue(/for the rollback path\./);
});

test("the account cluster does not cover Stop at the minimum window width", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.goto("/?live=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const stop = page.getByRole("button", { name: "Stop turn" });
  await expect(stop).toBeEnabled();
  await stop.click({ trial: true });
});

test("the composer exposes Build, Plan, Mission and Skills with context, queue and stop", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const modes = page.getByRole("group", { name: "Composer mode" });
  const build = modes.getByRole("button", { name: "Build", exact: true });
  const plan = modes.getByRole("button", { name: "Plan", exact: true });
  await expect(build).toHaveAttribute("aria-pressed", "true");
  await expect(modes.getByRole("button", { name: /Mission/ })).toBeVisible();
  await expect(modes.getByRole("button", { name: /Session skills/ })).toBeVisible();

  await expect(
    page.locator(".composer-controls").getByRole("button", { name: /^Context/ }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Local", exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: /Message queue/ }).click();
  await expect(page.getByRole("menu", { name: "Message queue" })).toContainText(
    "No prompts waiting",
  );

  await plan.click();
  await expect(plan).toHaveAttribute("aria-pressed", "true");
  await expect(build).toHaveAttribute("aria-pressed", "false");
});

test("while a preview turn runs, Send queues the draft and Stop is live", async ({ page }) => {
  await page.goto("/?live=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const composer = page.getByPlaceholder(/A turn is running/);
  await expect(composer).toBeEnabled();
  await composer.fill("Follow up after this turn");

  const send = page.getByRole("button", { name: "Queue message" });
  await expect(send).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeEnabled();
  await send.click();

  await expect(composer).toHaveValue("");
  const queue = page.getByRole("button", { name: "Message queue: 1 waiting" });
  await expect(queue).toBeVisible();
  await queue.click();
  await expect(page.getByRole("menu", { name: "Message queue" })).toContainText(
    "Follow up after this turn",
  );
});

// D153: the preflight is a range from recorded bills, shows only once the sample is big enough,
// and never takes Send away.
test("the cost preflight shows a range and leaves Send enabled", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const composer = page.getByPlaceholder(/Ask the agent/);
  await composer.fill("Add the same retry to the upload path");

  const preflight = page.locator(".composer-preflight");
  await expect(preflight).toBeVisible();
  await expect(preflight).toContainText(/≈ \$[\d.]+–\$[\d.]+ a turn/);
  await expect(preflight).toContainText("from 3 turns");

  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
  const quote = page.locator(".composer-quote");
  await expect(quote).toBeVisible();

  // Item 19: a price table past its freshness margin is visibly stale where the cost is shown,
  // naming how old it is and where it came from rather than silently presenting it as current.
  const stale = quote.locator(".price-stale");
  await expect(stale).toBeVisible();
  await expect(stale).toContainText("stale prices:");
  await expect(stale).toContainText(/\d+ days old/);
  await expect(stale).toContainText("vendor price pages");
});
