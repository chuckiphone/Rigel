import { expect, test } from "@playwright/test";

// Roadmap 144: the panel probes on demand, shows every declared server, and keeps a failed
// handshake as its own row with the error text rather than blocking the healthy one.
test("the MCP health panel shows ready and failed rows and refreshes on demand", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .getByRole("navigation", { name: "Primary" })
    .getByRole("button", { name: "Shelf", exact: true })
    .click();
  await page.getByRole("tab", { name: "MCPs" }).click();

  const panel = page.getByRole("region", { name: "MCP health" });
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId("mcp-health-summary")).toContainText("1 ready");
  await expect(panel.getByTestId("mcp-health-summary")).toContainText("1 failed");

  const ready = panel.locator(".mcp-health-row[data-state='ready']");
  const failed = panel.locator(".mcp-health-row[data-state='failed']");
  await expect(ready).toContainText("filesystem");
  await expect(ready).toContainText("11 tools");
  await expect(failed).toContainText("broken-server did not answer in time");

  const refresh = panel.getByRole("button", { name: "Refresh MCP health" });
  await expect(refresh).toBeEnabled();
  await refresh.click();
  await expect(refresh).toBeEnabled();
  await expect(ready).toBeVisible();
  await expect(failed).toBeVisible();
});
