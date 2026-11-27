import { expect, test } from "@playwright/test";

// Roadmap 145: a tool result is text, an image or a structured row. Long text is capped with a
// visible marker, and the exact recorded body is one click away.
test("MCP results render typed rows, cap long text and reveal raw on demand", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const rich = page.getByRole("region", { name: "MCP result · read_file" });
  await expect(rich).toBeVisible();
  await expect(rich.locator(".mcp-row.text")).toContainText("4,210 bytes read");
  await expect(rich.locator(".mcp-row.image img")).toBeVisible();
  await expect(rich.locator(".mcp-row.structured")).toHaveCount(3);
  await expect(rich.getByText("5 rows")).toBeVisible();

  const long = page.getByRole("region", { name: "MCP result · search_repo" });
  await expect(long).toBeVisible();
  await expect(long.getByText(/Truncated at 2000 characters · 1 more row hidden/)).toBeVisible();
  await expect(long.locator(".mcp-raw")).toHaveCount(0);
  await long.getByRole("button", { name: "Raw payload" }).click();
  await expect(long.locator(".mcp-raw")).toBeVisible();
  await expect(long.locator(".mcp-raw")).toContainText("match");
});

test("MCP rows keep their labels in the transcript and the raw body closes again", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const rich = page.getByRole("region", { name: "MCP result · read_file" });
  await expect(rich).toBeVisible();
  await expect(rich.locator(".mcp-row.structured").nth(1)).toContainText("path");
  await expect(rich.locator(".mcp-row.structured").last()).toContainText("bytes");

  const raw = rich.getByRole("button", { name: "Raw payload" });
  await raw.click();
  await expect(rich.locator(".mcp-raw")).toBeVisible();
  await raw.click();
  await expect(rich.locator(".mcp-raw")).toHaveCount(0);
  await expect(rich.locator(".mcp-row.image img")).toBeVisible();
});
