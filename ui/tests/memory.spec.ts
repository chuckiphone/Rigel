import { expect, test } from "@playwright/test";

// Roadmap 55: the browser preview ships sample facts, so the inspector's explanation is
// checked against real rendered rows rather than a unit test of the arithmetic alone.
test("the memory inspector shows recency, use and pin and can pin a fact", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const panel = page.getByRole("region", { name: "Memory facts" });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("Never force-push a shared branch.")).toBeVisible();

  const factors = panel.locator(".memory-factors").first();
  await expect(factors.getByText("recency", { exact: true })).toBeVisible();
  await expect(factors.getByText("use", { exact: true })).toBeVisible();
  await expect(factors.getByText("pin", { exact: true })).toBeVisible();
  await expect(factors).toContainText("1.00 × 0.50");

  // A pinned fact's reason states the promise the ranker makes.
  await expect(
    panel.getByText(/pinned: recency is held at full value and does not decay/).first(),
  ).toBeVisible();

  const toggle = panel.getByRole("button", { name: "unpin" }).first();
  await expect(toggle).toBeVisible();
  await toggle.click();
  await expect(panel.getByRole("button", { name: "pin" }).first()).toBeVisible();
});
