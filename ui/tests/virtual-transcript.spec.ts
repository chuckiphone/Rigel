import { expect, test } from "@playwright/test";

// Roadmap 186: a 10,000-turn preview keeps only the visible slice in the DOM, keeps the centred
// row centred when the viewport shrinks, and lands a jump on the exact row.
test("the virtual transcript keeps its anchor on resize and jumps to the named row", async ({
  page,
}) => {
  await page.goto("/?virtual=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const list = page.getByRole("region", { name: "Virtual transcript" });
  await expect(list).toBeVisible();
  const rows = list.locator(".virtual-row");
  const count = await rows.count();
  expect(count).toBeGreaterThan(0);
  expect(count).toBeLessThan(60);
  await expect(page.getByText("10,000 turns · only the visible window is in the DOM")).toBeVisible();

  await list.evaluate((element) => {
    element.scrollTop = 200_000;
  });
  await expect(list.locator('.virtual-row[data-index="4996"]')).toBeVisible();
  const centeredBefore = await list.evaluate(
    (element) => Math.floor((element.scrollTop + element.clientHeight / 2) / 40),
  );

  // Shrink the viewport under the same anchored row.
  await list.evaluate((element) => {
    element.style.maxHeight = "180px";
  });
  await expect.poll(async () =>
    list.evaluate((element) => Math.floor((element.scrollTop + element.clientHeight / 2) / 40)),
  ).toBe(centeredBefore);
  expect(await rows.count()).toBeLessThan(count + 10);

  await page.getByRole("button", { name: "Jump to turn 9000" }).click();
  await expect(list.locator('.virtual-row[data-index="8999"]')).toBeVisible();
});
