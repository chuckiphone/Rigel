import { expect, test } from "@playwright/test";

// Roadmap 179: the preview's `?cloud=1` fixture is signed in so the account page's publish gate
// can be exercised without a Firebase account.

test("base, ours and theirs are shown and an unresolved field blocks publishing", async ({
  page,
}) => {
  await page.goto("/?cloud=1");
  await page.getByRole("button", { name: "Account", exact: true }).click();

  const panel = page.getByRole("region", { name: "Metadata conflicts" });
  await expect(panel).toBeVisible();
  const field = panel.locator('[data-field="publish_scope"]');
  await expect(field).toContainText("turns");
  await expect(field).toContainText("all");
  await expect(field).toContainText("sessions");
  for (const side of ["Base", "Ours", "Theirs"]) {
    await expect(field).toContainText(side);
  }
  await expect(panel.getByRole("alert")).toContainText(
    "Publishing is blocked until publish_scope is resolved",
  );
  await expect(page.getByRole("button", { name: "Publish all sessions" })).toBeDisabled();
});

test("a recorded decision carries the author and unlocks publishing", async ({ page }) => {
  await page.goto("/?cloud=1");
  await page.getByRole("button", { name: "Account", exact: true }).click();

  const panel = page.getByRole("region", { name: "Metadata conflicts" });
  await panel.getByRole("radio", { name: "Theirs for publish_scope" }).check();
  await panel.getByRole("button", { name: "Record decision" }).click();

  await expect(panel).toContainText("theirs chosen by reviewer@example.com at");
  await expect(panel.getByRole("status")).toContainText(
    "Every conflict has a recorded decision",
  );
  await expect(page.getByRole("button", { name: "Publish all sessions" })).toBeEnabled();
});
