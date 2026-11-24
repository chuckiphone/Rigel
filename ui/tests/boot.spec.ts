import { expect, test } from "@playwright/test";

// The intro is skipped for automation so the existing surface tests keep their first-paint
// contract. `?boot=1` is the deliberate replay path used here and for a visual check.
test("the boot intro plays on demand and skips into the app", async ({ page }) => {
  await page.goto("/?boot=1");
  const intro = page.locator(".boot");

  await expect(intro).toBeVisible();
  await expect(page.locator(".boot-canvas")).toBeVisible();

  await page.getByRole("button", { name: "Skip intro" }).click();
  await expect(intro).toHaveCount(0, { timeout: 3_000 });
  await expect(page.getByRole("heading", { name: "The Rigel Paper" })).toBeVisible();
});

test.describe("reduced motion", () => {
  test.use({ reducedMotion: "reduce" });

  test("the intro leaves a still mark and clears itself", async ({ page }) => {
    await page.goto("/?boot=1");
    await expect(page.locator(".boot")).toBeVisible();

    await expect(page.locator(".boot")).toHaveCount(0, { timeout: 3_000 });
    await expect(page.getByRole("heading", { name: "The Rigel Paper" })).toBeVisible();
  });
});
