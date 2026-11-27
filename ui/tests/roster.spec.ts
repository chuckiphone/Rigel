import { expect, test } from "@playwright/test";

// Roadmap 131: the browser-only 500-session fixture exercises the same virtualised roster the
// desktop list uses. Every session must be reachable by scrolling; only a window is mounted.

async function openRoster(page: import("@playwright/test").Page) {
  await page.goto("/?roster=500");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "sessions" }).click();
  return page.getByRole("region", { name: "Session roster" });
}

test("500 sessions scroll without dropping a row", async ({ page }) => {
  const roster = await openRoster(page);
  await expect(roster).toBeVisible();
  expect(await roster.locator("[data-session]").count()).toBeLessThan(40);

  const seen = await roster.evaluate(async (element) => {
    const ids = new Set<string>();
    const collect = () => {
      element.querySelectorAll("[data-session]").forEach((row) => {
        const id = row.getAttribute("data-session");
        if (id) ids.add(id);
      });
    };
    collect();
    for (let top = 0; top <= element.scrollHeight; top += 180) {
      element.scrollTop = top;
      await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      collect();
    }
    element.scrollTop = element.scrollHeight;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    collect();
    return [...ids];
  });

  expect(seen.length).toBe(500);
  expect(seen).toContain("fixture-session-0497");
  expect(seen).toContain("fixture-session-0500");
  await expect(roster.locator("[data-session]").first()).toBeVisible();
});

test("the row menu opens and walks its actions from the keyboard", async ({ page }) => {
  const roster = await openRoster(page);
  const trigger = roster.getByRole("button", {
    name: "Actions for Fixture session 0497",
  });
  await trigger.focus();
  await page.keyboard.press("ArrowDown");

  const menu = roster.getByRole("menu", { name: "Actions for Fixture session 0497" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "Open session" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "Pin to the top" })).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "Rename Fixture session 0497" })).toBeFocused();
  await expect(menu.getByRole("menuitem", { name: /Delete permanently/ })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
});
