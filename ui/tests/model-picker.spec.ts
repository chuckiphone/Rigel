import { expect, test } from "@playwright/test";

// Roadmap 134: grouping, search and the free filter narrow the roster; hints are facts only.
async function openPicker(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.locator(".session-tools .model-button").click();
  return page.getByRole("dialog", { name: "Model and reasoning effort" });
}

test("the picker groups by provider and searches and filters within the groups", async ({ page }) => {
  const dialog = await openPicker(page);
  const labels = await dialog.locator(".model-group-label").allTextContents();
  expect(labels.some((label) => label.startsWith("openrouter"))).toBe(true);
  expect(labels.some((label) => label.startsWith("omniroute"))).toBe(true);
  expect(labels.some((label) => label.startsWith("local"))).toBe(true);

  const doors = dialog.getByRole("group", { name: "Providers in this list" });
  await doors.getByRole("button", { name: /^openrouter/ }).click();
  const rows = dialog.locator(".model-row");
  expect(await rows.count()).toBeGreaterThan(0);
  for (let index = 0; index < (await rows.count()); index += 1) {
    await expect(rows.nth(index).locator(".model-tags .tag").last()).toHaveText("openrouter");
  }
  expect(await dialog.locator(".model-group-label").allTextContents()).toEqual([
    expect.stringContaining("openrouter"),
  ]);

  await doors.getByRole("button", { name: /^all/ }).click();
  await dialog.getByRole("textbox", { name: "Search models" }).fill("claude");
  await expect(dialog.locator(".model-row")).toHaveCount(1);
  await expect(dialog.locator(".model-row")).toContainText("claude");
  await dialog.getByRole("textbox", { name: "Search models" }).fill("");

  await dialog.getByRole("button", { name: "free only" }).click();
  const freeRows = dialog.locator(".model-row-wrap");
  expect(await freeRows.count()).toBeGreaterThan(0);
  for (let index = 0; index < (await freeRows.count()); index += 1) {
    await expect(freeRows.nth(index).locator(".tag.free")).toBeVisible();
  }
});

test("the resolved pair appears on hover and unknown price and latency stay unknown", async ({
  page,
}) => {
  const dialog = await openPicker(page);
  const row = dialog.locator(".model-row-wrap", { hasText: "gpt-5.6-sol-medium" }).first();
  await expect(row.locator(".model-price-hint")).toContainText("price · unknown");
  await expect(row.locator(".model-latency-hint")).toContainText("latency · unknown");
  await expect(row.locator(".tag.model-cost.unknown")).toHaveText("unknown");

  const pair = row.locator(".model-pair");
  await expect(pair).toHaveText("omniroute / gpt-5.6-sol-medium");
  await expect(row.locator(".model-row")).toHaveAttribute(
    "aria-describedby",
    await pair.getAttribute("id"),
  );
  await expect(pair).toHaveCSS("opacity", "0");
  await row.locator(".model-row").hover();
  await expect(pair).toHaveCSS("opacity", "1");
});
