import { expect, test } from "@playwright/test";

// Roadmap 113: one palette reaches actions, files, symbols, sessions and settings; an unknown
// slash command is a named error and never becomes a turn.

test("the palette shows every roadmap section", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: /^Search/ }).click();
  const dialog = page.getByRole("dialog", { name: /Search actions/ });
  const search = page.getByRole("textbox", { name: "Search Rigel" });

  await expect(dialog.locator(".palette-section").filter({ hasText: "Actions" })).toBeVisible();

  await search.fill("word");
  await expect(dialog.locator(".palette-section").filter({ hasText: "Files" })).toBeVisible();
  await expect(dialog.locator(".palette-section").filter({ hasText: "Symbols" })).toBeVisible();
  await expect(dialog.getByRole("option", { name: /word_diff/ })).toBeVisible();

  await search.fill("provid");
  await expect(dialog.locator(".palette-section").filter({ hasText: "Settings" })).toBeVisible();
  await expect(dialog.getByRole("option", { name: /Providers/ }).first()).toBeVisible();

  await search.fill("flaky");
  await expect(dialog.locator(".palette-section").filter({ hasText: "Sessions" })).toBeVisible();
  await expect(dialog.getByRole("option", { name: /flaky resume test/ })).toBeVisible();
});

test("an unknown slash command is a named error, never a prompt", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  const composer = page.getByPlaceholder(/Ask the agent/);
  await composer.fill("/frobnicate now");
  await page.getByRole("button", { name: "Send", exact: true }).click();

  await expect(page.locator("p.notice[role=status]")).toContainText(/Unknown command \/frobnicate/);
  await expect(page.getByRole("region", { name: "Transcript" })).not.toContainText("frobnicate");
  await expect(composer).toHaveValue("/frobnicate now");

  await composer.fill("/help");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(composer).toHaveValue("");
});
