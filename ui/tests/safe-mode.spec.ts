import { expect, test } from "@playwright/test";

// Roadmap 82: every turn row in an untrusted session carries the mode, and the only way out is
// the typed phrase the core enforces.
test("safe mode marks every turn and needs the typed phrase to leave", async ({ page }) => {
  await page.goto("/?safe=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const panel = page.getByRole("region", { name: "Safe mode" });
  await expect(panel).toHaveAttribute("data-mode", "safe");
  await expect(panel).toContainText("Tools, network and scripts are off");

  const rows = page.locator("article.turn");
  const count = await rows.count();
  expect(count).toBeGreaterThan(0);
  await expect(page.locator("article.turn .turn-mode.safe")).toHaveCount(count);

  const leave = panel.getByRole("button", { name: "Leave safe mode" });
  await expect(leave).toBeDisabled();
  const confirmation = panel.getByRole("textbox", { name: "Safe mode confirmation" });
  await confirmation.fill("wrong phrase");
  await expect(leave).toBeDisabled();
  await confirmation.fill("leave safe mode");
  await expect(leave).toBeEnabled();
  await leave.click();

  await expect(panel).toHaveAttribute("data-mode", "normal");
  // The rows already recorded keep the mode they ran under: leaving is not a rewrite of history.
  await expect(page.locator("article.turn .turn-mode.safe")).toHaveCount(count);
  await expect(panel.getByRole("button", { name: "Open this session in safe mode" })).toBeVisible();
});
