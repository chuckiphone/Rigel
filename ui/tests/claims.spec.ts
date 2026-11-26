import { expect, test } from "@playwright/test";

// Roadmap 95: the browser preview ships a held claim and a stale one so the ledger states are
// inspectable without a native host. The browser cannot release anything; that is the point.
test("the claim dashboard shows counts, a stale claim's age and an explicit release", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Diff", exact: true }).click();

  const dashboard = page.getByRole("region", { name: "Claim dashboard" });
  await expect(dashboard).toBeVisible();
  await expect(
    dashboard.getByText("2 claims · 2 held · 1 stale · 2 holders", { exact: false }),
  ).toBeVisible();
  await expect(dashboard.getByText(/Stale heartbeat is reported, never released automatically/i)).toBeVisible();

  const staleRow = dashboard.locator(".claim-dashboard-row.stale");
  await expect(staleRow).toContainText("ui/src/shell.css");
  await expect(staleRow).toContainText("reviewer");
  await expect(staleRow.getByTitle("last heartbeat")).toHaveText("5m ago");

  // Reading a row is not releasing it: the claim stays stale until the explicit action.
  await staleRow.locator(".claim-path").click();
  await expect(staleRow.locator(".pill")).toHaveText("stale");

  await staleRow.getByRole("button", { name: "Release…" }).click();
  await staleRow.getByRole("button", { name: "Release claim" }).click();
  await expect(dashboard.getByRole("status")).toContainText("at least 8 characters");
  await expect(staleRow.locator(".pill")).toHaveText("stale");

  await staleRow.getByPlaceholder("why this claim is released").fill("handed back to the writer");
  await staleRow.getByRole("button", { name: "Release claim" }).click();
  await expect(dashboard.getByRole("status")).toContainText(
    "Preview is read-only: no claim was released.",
  );
  await expect(staleRow.locator(".pill")).toHaveText("stale");
});
