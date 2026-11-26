import { expect, test } from "@playwright/test";

// Roadmap 117: an empty surface is a control. Both surfaces below reach their empty state from a
// filter, so the one action can be clicked and its effect observed; neither draws an illustration.
test("an empty session list clears its filter from the empty-state control", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "sessions" }).click();

  const filter = page.getByRole("textbox", { name: "Filter sessions" });
  await filter.fill("nothing matches");
  const empty = page.locator('.empty-state[data-surface="sessions"]');
  await expect(empty).toBeVisible();
  await expect(empty.locator("svg")).toHaveCount(0);
  await empty.getByRole("button", { name: "Clear filters" }).click();

  await expect(filter).toHaveValue("");
  await expect(page.locator(".session-row").first()).toBeVisible();
});

test("an empty plugin filter clears itself from the empty-state control", async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Plugins/ }).click();

  const search = page.getByRole("textbox", { name: "Search the plugin catalogue" });
  await search.fill("no-such-plugin-xyz");
  const empty = page.locator('.empty-state[data-surface="plugins"]');
  await expect(empty).toBeVisible();
  await expect(empty.locator("svg")).toHaveCount(0);
  await empty.getByRole("button", { name: "Clear filters" }).click();

  await expect(search).toHaveValue("");
  await expect(page.getByText("No catalogue entries in this build.")).toBeVisible();
});
