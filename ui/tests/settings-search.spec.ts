import { expect, test } from "@playwright/test";

// Roadmap 125: a settings address names the exact row, not just the page. An unknown key says so.
test("a deep link opens the page and focuses the named setting", async ({ page }) => {
  await page.goto("/#settings/appearance?focus=theme");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();

  await expect(page.getByRole("heading", { name: "Appearance" })).toBeVisible();
  const focusedKey = await page.evaluate(() =>
    document.activeElement?.getAttribute("data-setting-key"),
  );
  expect(focusedKey).toBe("theme");
  await expect.poll(() => page.evaluate(() => window.location.hash)).toContain("focus=theme");
});

test("an unknown setting key shows a not-found state on the known page", async ({ page }) => {
  await page.goto("/#settings/appearance?focus=not-a-setting");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();

  await expect(page.getByRole("heading", { name: "Appearance" })).toBeVisible();
  await expect(page.getByRole("status")).toContainText("No setting named");
  await expect(page.getByRole("status")).toContainText("not-a-setting");
});

test("a search result opens its page and focuses the exact row", async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();

  await page.getByRole("textbox", { name: "Search settings" }).fill("theme");
  const result = page
    .getByRole("group", { name: "Matching settings" })
    .getByRole("option", { name: /^Theme/ });
  await expect(result).toBeVisible();
  await result.click();

  await expect(page.getByRole("heading", { name: "Appearance" })).toBeVisible();
  const focusedKey = await page.evaluate(() =>
    document.activeElement?.getAttribute("data-setting-key"),
  );
  expect(focusedKey).toBe("theme");
  await expect.poll(() => page.evaluate(() => window.location.hash)).toContain("focus=theme");
});
