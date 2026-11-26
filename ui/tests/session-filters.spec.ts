import { expect, test } from "@playwright/test";

// Roadmap 102: the filter is the address. A pasted hash restores the same controls, an unknown
// tag stays as plain text, and a saved filter is still there after a reload.
test("a session filter round-trips through the URL and unknown tags stay plain text", async ({
  page,
}) => {
  await page.goto("/#sessions?query=flaky&status=active&tag=made-up");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "sessions" }).click();

  await expect(page.getByRole("textbox", { name: "Filter sessions" })).toHaveValue("flaky");
  await expect(page.getByRole("combobox", { name: "Filter sessions by status" })).toHaveValue(
    "active",
  );
  const tags = page.getByRole("group", { name: "Current filter tags" });
  const unknown = tags.getByRole("button", { name: "Remove tag made-up" });
  await expect(unknown).toBeVisible();
  await expect(unknown).toHaveText("made-up");
  await expect(unknown).toHaveClass(/filter-tag/);
  await expect(unknown).not.toHaveClass(/known/);
  await expect(page.getByRole("alert")).toHaveCount(0);

  await page.getByRole("textbox", { name: "Filter sessions" }).fill("changed");
  await expect.poll(() => page.evaluate(() => window.location.hash)).toContain("query=changed");
  await expect.poll(() => page.evaluate(() => window.location.hash)).toContain("tag=made-up");
});

test("a named filter survives a reload", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "sessions" }).click();

  const region = page.getByRole("region", { name: "Tags and saved filters" });
  await region.getByRole("textbox", { name: "Filter name" }).fill("review");
  await region.getByRole("button", { name: "Save filter" }).click();
  await expect(region.getByRole("button", { name: "Apply saved filter review" })).toBeVisible();

  await page.reload();
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "sessions" }).click();
  await expect(
    page
      .getByRole("region", { name: "Tags and saved filters" })
      .getByRole("button", { name: "Apply saved filter review" }),
  ).toBeVisible();
});
