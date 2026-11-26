import { expect, test } from "@playwright/test";

// Roadmap 137: one row at the minimum width, single-line labels, and a keyboard path through it.
test("the composer row fits 320px with no wrapped label and every control reachable", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 900 });
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  // Focus mode hides the rail and list at the minimum width so the work surface has the full pane.
  await page.locator(".focus-chip").click();

  const row = page.getByRole("group", { name: "Composer controls" });
  await expect(row).toBeVisible();
  for (const name of [/^Attach /, /^Skills /, /^Model /, /^Context /]) {
    await expect(row.getByRole("button", { name })).toBeVisible();
  }
  await expect(row.getByRole("combobox", { name: "Composer effort" })).toBeVisible();
  await expect(row.getByRole("button", { name: "Act mode" })).toBeVisible();
  await expect(row.getByRole("button", { name: "Plan mode" })).toBeVisible();

  const labels = row.locator(".composer-v2-label");
  for (let index = 0; index < (await labels.count()); index += 1) {
    const label = labels.nth(index);
    expect(await label.evaluate((element) => getComputedStyle(element).whiteSpace)).toBe("nowrap");
    const box = await label.boundingBox();
    expect(box, "label has a box").not.toBeNull();
    expect(box!.height, "label stays on one line").toBeLessThan(18);
  }

  const controls = row.locator(".composer-v2-control");
  for (let index = 0; index < (await controls.count()); index += 1) {
    const box = await controls.nth(index).boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(320);
  }

  // Tab reaches each native control from the first button.
  await row.getByRole("button", { name: /^Attach / }).focus();
  const reached: string[] = [];
  for (let step = 0; step < 7; step += 1) {
    await page.keyboard.press("Tab");
    reached.push(
      await page.evaluate(() => {
        const active = document.activeElement as HTMLElement | null;
        return (active?.getAttribute("aria-label") ?? active?.textContent ?? "").trim();
      }),
    );
  }
  for (const name of ["Skills", "Model", "Composer effort", "Context", "Plan mode"]) {
    expect(reached.some((value) => value.startsWith(name))).toBe(true);
  }
});

test("the v2 controls open the surfaces they name and toggle the plan", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  const row = page.getByRole("group", { name: "Composer controls" });

  await row.getByRole("button", { name: /^Attach / }).click();
  await expect(page.getByRole("menu", { name: "Add context" })).toBeVisible();
  await page.keyboard.press("Escape");

  await row.getByRole("button", { name: /^Context / }).click();
  await expect(page.getByRole("dialog", { name: "Context window" })).toBeVisible();
  await page.keyboard.press("Escape");

  await row.getByRole("button", { name: "Plan mode" }).click();
  await expect(row.getByRole("button", { name: "Plan mode" })).toHaveAttribute("aria-pressed", "true");
  await expect(row.getByRole("button", { name: "Act mode" })).toHaveAttribute("aria-pressed", "false");
  await row.getByRole("button", { name: "Act mode" }).click();
  await expect(row.getByRole("button", { name: "Act mode" })).toHaveAttribute("aria-pressed", "true");

  await row.getByRole("combobox", { name: "Composer effort" }).selectOption("high");
  await expect(row.getByRole("combobox", { name: "Composer effort" })).toHaveValue("high");
});
