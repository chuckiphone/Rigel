import { expect, test } from "@playwright/test";

// Roadmap 108: `?setup=1` renders the wizard in the shell; `?slow=1` makes a probe pending long
// enough to prove progress does not move before the finding arrives.

test("progress stays put until a check completes, then shows the real finding", async ({ page }) => {
  await page.goto("/?setup=1&slow=1");
  await page.getByRole("button", { name: "Shelf", exact: true }).click();
  const wizard = page.getByRole("region", { name: "First-run setup" });
  const progress = wizard.getByRole("progressbar", { name: "Setup progress" });
  await expect(progress).toHaveAttribute("value", "0");

  const osStep = wizard.locator('[data-step="os"]');
  await osStep.getByRole("button", { name: "Run check" }).click();
  await expect(osStep.getByRole("button", { name: /Checking/ })).toBeDisabled();
  await expect(progress).toHaveAttribute("value", "0", "a pending probe is not progress");

  await expect(osStep.getByLabel("Finding for Operating system")).toBeVisible();
  await expect(osStep.getByLabel("Finding for Operating system")).toContainText("Mozilla");
  await expect(progress).toHaveAttribute("value", "1");
});

test("every step can be skipped and each skip counts as an answer", async ({ page }) => {
  await page.goto("/?setup=1");
  await page.getByRole("button", { name: "Shelf", exact: true }).click();
  const wizard = page.getByRole("region", { name: "First-run setup" });
  for (const step of ["os", "shell", "git", "providers", "harnesses"]) {
    const row = wizard.locator(`[data-step="${step}"]`);
    await expect(row.getByRole("button", { name: "Skip" })).toBeEnabled();
    await row.getByRole("button", { name: "Skip" }).click();
    await expect(row.getByLabel(/Finding for/)).toContainText("skipped by the user");
  }
  await expect(wizard.getByRole("progressbar", { name: "Setup progress" })).toHaveAttribute("value", "5");
  await expect(wizard).toContainText("Every step has an answer.");
  await wizard.getByRole("button", { name: "Finish setup" }).click();
  await expect(wizard).toHaveCount(0);
});

test("a run check reports what the machine actually exposes", async ({ page }) => {
  await page.goto("/?setup=1");
  await page.getByRole("button", { name: "Shelf", exact: true }).click();
  const wizard = page.getByRole("region", { name: "First-run setup" });
  const gitStep = wizard.locator('[data-step="git"]');
  await gitStep.getByRole("button", { name: "Run check" }).click();
  await expect(gitStep.getByLabel("Finding for Git")).toContainText("browser preview");

  const harnessStep = wizard.locator('[data-step="harnesses"]');
  await harnessStep.getByRole("button", { name: "Run check" }).click();
  await expect(harnessStep.getByLabel("Finding for Harnesses")).toContainText("codex");
});
