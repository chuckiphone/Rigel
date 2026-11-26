import { expect, test } from "@playwright/test";

import { PREVIEW_SPEND, costCsv, costJson } from "../src/cost-dashboard";

// Roadmap 136: the dashboard reconciles every grouping with the stored total and exports the
// same report object, down to the micro-dollar.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        async writeText(text: string) {
          (window as unknown as { __copied: string }).__copied = text;
        },
      },
    });
  });
});

test("Spending shows a reconciled cost dashboard with CSV and JSON export", async ({ page }) => {
  await page.goto("/?cost=1");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Spending/ }).click();

  const dashboard = page.getByRole("region", { name: "Cost dashboard" });
  await expect(dashboard).toBeVisible();
  await expect(dashboard.locator(".cost-total")).toHaveText("$0.12");
  await expect(dashboard.getByText(/Totals reconcile: day, model, agent and session all sum to \$0\.12/)).toBeVisible();
  for (const group of ["by day", "by model", "by agent"]) {
    await expect(dashboard.getByRole("heading", { name: group })).toBeVisible();
  }
  await expect(dashboard.getByRole("table")).toContainText("openrouter/fast");
  await expect(dashboard.getByText("No daily cap set for this project.")).toBeVisible();

  await dashboard.getByRole("button", { name: "Copy JSON" }).click();
  const copiedJson = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(JSON.parse(copiedJson)).toEqual(PREVIEW_SPEND);
  expect(copiedJson).toBe(costJson(PREVIEW_SPEND));
  const report = JSON.parse(copiedJson);
  for (const key of ["by_day", "by_model", "by_agent", "by_session"]) {
    expect(report[key].reduce((sum: number, row: { micros: number }) => sum + row.micros, 0)).toBe(
      report.total_micros,
    );
  }

  await dashboard.getByRole("button", { name: "Copy CSV" }).click();
  const copiedCsv = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(copiedCsv).toBe(costCsv(PREVIEW_SPEND));
});
