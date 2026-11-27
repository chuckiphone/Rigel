import { expect, test } from "@playwright/test";

// Roadmap 104: the comparison is opened from the session toolbar and uses the same recorded turn
// data as the transcript, so both columns name the same real fields.

async function openCompare(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "Compare turns" }).click();
  return page.getByRole("region", { name: "Compare turns" });
}

test("comparison opens side by side, names the turns and highlights differing fields", async ({
  page,
}) => {
  const compare = await openCompare(page);
  await expect(compare).toBeVisible();
  await expect(compare).toContainText(/Turn 1 vs turn/);
  await expect(compare.locator(".compare-column")).toHaveCount(2);
  for (const label of ["Prompt", "Route", "Served model", "Tool calls", "Output", "Cost"]) {
    await expect(compare.locator(".compare-row").filter({ hasText: label })).toHaveCount(2);
  }
  await expect(compare.locator(".compare-row.differing").first()).toBeVisible();
  await expect(compare).toContainText(/fields differ/);
});

test("both columns keep one scroll anchor and the comparison can be closed", async ({ page }) => {
  const compare = await openCompare(page);
  const columns = compare.locator(".compare-column");
  await expect(columns).toHaveCount(2);

  // Scrolling one column moves the other to the same anchored row.
  await columns.first().evaluate((element) => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await expect
    .poll(async () => columns.last().evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  const a = await columns.first().evaluate((element) => element.scrollTop);
  const b = await columns.last().evaluate((element) => element.scrollTop);
  expect(Math.abs(a - b)).toBeLessThan(2);

  await page.getByRole("button", { name: "Close turn comparison" }).click();
  await expect(compare).toHaveCount(0);
});
