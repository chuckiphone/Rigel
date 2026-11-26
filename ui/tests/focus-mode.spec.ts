import { expect, test } from "@playwright/test";

// Roadmap 120: focus hides the chrome, never the work. The composer and the approval card live in
// the detail pane, so they stay reachable while the rail and list are gone.
test("focus keeps the composer and approvals while hiding the rail and list", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const chip = page.locator(".focus-chip");
  await chip.click();
  await expect(chip).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".shell")).toHaveClass(/focus/);
  await expect(page.locator(".rail")).toBeHidden();
  await expect(page.locator(".list-pane").first()).toBeHidden();

  await expect(page.getByPlaceholder(/Ask the agent/)).toBeVisible();
  await expect(page.getByRole("region", { name: "Approval required" })).toBeVisible();

  await chip.click();
  await expect(chip).toHaveAttribute("aria-pressed", "false");
  await expect(page.locator(".rail")).toBeVisible();
});
