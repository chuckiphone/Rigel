import { expect, test } from "@playwright/test";

// Roadmap 58: the roster row, not the window, owns the model pair. The browser preview has no
// role presets or Rust commands, so this checks the rendered picker and the resolved `next` chip;
// the preset override and the stored pair are covered by the core tests.
test("each roster row owns a model picker and the next chip shows the pair", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const rows = page.locator(".agent-row");
  await expect(rows.first()).toBeVisible();
  const count = await rows.count();
  expect(count).toBeGreaterThan(0);

  for (let index = 0; index < count; index += 1) {
    await expect(rows.nth(index).locator(".agent-row-config .model-button")).toBeVisible();
  }

  const first = rows.first();
  await expect(first.locator(".row-desc")).toContainText("anthropic/claude-sonnet-4.5");
  await expect(first.getByRole("combobox", { name: /Role preset/ })).toHaveValue("worker");

  const next = page.locator(".resolved-route");
  await expect(next).toContainText("anthropic/claude-sonnet-4.5");
  await expect(next).toContainText("agent row");

  await first.locator(".agent-row-config .model-button").click();
  const picker = page.getByRole("dialog", { name: "Model and reasoning effort" });
  await expect(picker).toBeVisible();
  await expect(picker).toContainText("Model for worker");
  await page.keyboard.press("Escape");
  await expect(picker).toBeHidden();
});
