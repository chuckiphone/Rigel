import { expect, test } from "@playwright/test";

// Roadmap 114: the routing table is visible in Settings, and every event has one destination.

test("settings shows the notification routing table with all three destinations", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("option", { name: /Notifications/ }).click();

  const table = page.getByRole("table", { name: "Notification routing table" });
  await expect(table).toBeVisible();
  await expect(table.locator("tbody tr")).toHaveCount(3);
  await expect(table.locator('tr[data-kind="approval"]')).toContainText("In-app");
  await expect(table.locator('tr[data-kind="finished"]')).toContainText("Tray");
  await expect(table.locator('tr[data-kind="failed"]')).toContainText("Tray");
  await expect(table.locator("tbody")).toContainText("enabled");
});
