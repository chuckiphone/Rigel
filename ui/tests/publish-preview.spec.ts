import { expect, test } from "@playwright/test";

// Roadmap 164: the preview lists every row before publish, and redaction switches are per session.

async function openPreview(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "Publication preview" }).click();
  return page.getByRole("region", { name: "Publication preview" });
}

test("every published row is listed with tool output redacted by default", async ({ page }) => {
  const panel = await openPreview(page);
  await expect(panel).toBeVisible();
  for (const label of ["Session header", "prompt", "assistant reply", "cost and tokens"]) {
    await expect(panel.locator("tbody")).toContainText(label);
  }
  await expect(panel.locator('tr[data-kind="tool_output"]').first()).toContainText(
    "[tool output withheld] · redacted",
  );
  await expect(panel.locator('tr[data-kind="diff"]').first()).toContainText(
    "[diff withheld] · redacted",
  );
  await expect(panel.getByRole("checkbox", { name: "Include tool output" })).not.toBeChecked();
  await expect(panel.getByRole("checkbox", { name: "Include diffs" })).not.toBeChecked();
});

test("opting into tool output reveals it and closes hides the panel", async ({ page }) => {
  const panel = await openPreview(page);
  await panel.getByRole("checkbox", { name: "Include tool output" }).check();
  await expect(panel.locator('tr[data-kind="tool_output"]').first()).not.toContainText(
    "[tool output withheld]",
  );
  await expect(panel.getByRole("checkbox", { name: "Include diffs" })).not.toBeChecked();
  await panel.getByRole("button", { name: "Close publication preview" }).click();
  await expect(panel).toHaveCount(0);
});

test("the chosen scope is stored for this session only", async ({ page }) => {
  const panel = await openPreview(page);
  await panel.getByRole("checkbox", { name: "Include tool output" }).check();
  await expect(panel.getByRole("checkbox", { name: "Include tool output" })).toBeChecked();

  // Reopening reads the stored scope rather than falling back to the default.
  await panel.getByRole("button", { name: "Close publication preview" }).click();
  await page.getByRole("button", { name: "Publication preview" }).click();
  await expect(
    page.getByRole("region", { name: "Publication preview" }).getByRole("checkbox", { name: "Include tool output" }),
  ).toBeChecked();
});
