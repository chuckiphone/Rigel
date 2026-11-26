import { expect, test } from "@playwright/test";

// Roadmap 124: the browser preview has no build stamp, so About says `preview-build` instead of
// inventing one. The licence list is the one generated from the release SBOM and checked in.
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

test("the About dialog names the build, the SBOM licences and a copyable diagnostics block", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "About", exact: true }).click();

  const dialog = page.getByRole("dialog", { name: "About Rigel" });
  await expect(dialog).toBeVisible();
  await expect(dialog.locator(".pill").filter({ hasText: /^preview$/ })).toBeVisible();
  await expect(
    dialog.getByRole("definition").filter({ hasText: /^preview-build$/ }),
  ).toBeVisible();
  await expect(
    dialog
      .getByRole("definition")
      .filter({ hasText: /MIT OR Apache-2\.0 · \d+ SBOM components/ }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("definition").filter({ hasText: /signed GitHub releases\/latest/ }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("definition").filter({ hasText: /No telemetry/ }),
  ).toBeVisible();
  await expect(dialog.getByText(/Dependency licence expressions \(\d+\)/)).toBeVisible();

  const block = await dialog.getByLabel("Diagnostics block").innerText();
  expect(block).toContain("commit preview-build");
  await dialog.getByRole("button", { name: "Copy diagnostics" }).click();
  const copied = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(copied).toBe(block);
});

test("Settings has the same About facts without the modal shell", async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^About/ }).click();

  await expect(page.getByRole("heading", { name: "About Rigel" })).toBeVisible();
  await expect(
    page.getByRole("definition").filter({ hasText: /^preview-build$/ }),
  ).toBeVisible();
  await expect(
    page
      .getByRole("definition")
      .filter({ hasText: /MIT OR Apache-2\.0 · \d+ SBOM components/ }),
  ).toBeVisible();
  await expect(page.getByLabel("Diagnostics block")).toContainText("commit preview-build");
  await expect(page.getByRole("dialog", { name: "About Rigel" })).toHaveCount(0);
});
