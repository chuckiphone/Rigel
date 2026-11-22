import { expect, test, type Locator } from "@playwright/test";

type Box = { x: number; y: number; width: number; height: number };

const measured = async (locator: Locator): Promise<Box> => {
  const box = await locator.boundingBox();
  expect(box, "the element must be rendered").not.toBeNull();
  return box as Box;
};

const overlaps = (a: Box, b: Box) =>
  a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y;

const VIEWPORTS = [
  { width: 1280, height: 800 },
  { width: 720, height: 640 },
];

for (const viewport of VIEWPORTS) {
  test(`the utility cluster stays bottom-right at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await page.goto("/");

    const cluster = page.getByRole("group", { name: "Account and settings" });
    await expect(cluster).toBeVisible();
    const account = cluster.getByRole("button", { name: "Account", exact: true });
    const settings = cluster.getByRole("button", { name: "Settings", exact: true });
    const rail = page.getByRole("navigation", { name: "Primary" });
    const health = page.locator(".rail-status");
    const footer = page.locator("footer.status");

    await expect(account).toBeVisible();
    await expect(settings).toBeVisible();
    await expect(health).toBeVisible();
    await expect(footer).toBeVisible();

    const [accountBox, settingsBox, railBox, healthBox, footerBox] = await Promise.all([
      measured(account),
      measured(settings),
      measured(rail),
      measured(health),
      measured(footer),
    ]);

    // Settings is directly above Account, and both share the same right edge.
    const gap = accountBox.y - (settingsBox.y + settingsBox.height);
    expect(gap, "the pair should read as one cluster").toBeGreaterThanOrEqual(4);
    expect(gap, "the pair should read as one cluster").toBeLessThanOrEqual(12);
    expect(
      Math.abs(
        settingsBox.x + settingsBox.width - (accountBox.x + accountBox.width),
      ),
      "Settings and Account are right-aligned",
    ).toBeLessThanOrEqual(1);

    // Anchored to the viewport, not the rail: 28px content inset and 40px above the status bar.
    const rightInset = viewport.width - (accountBox.x + accountBox.width);
    const bottomInset = viewport.height - (accountBox.y + accountBox.height);
    expect(rightInset, "the pair uses the normal right content inset").toBeGreaterThanOrEqual(24);
    expect(rightInset, "the pair uses the normal right content inset").toBeLessThanOrEqual(32);
    expect(bottomInset, "the pair sits just above the status bar").toBeGreaterThanOrEqual(36);
    expect(bottomInset, "the pair sits just above the status bar").toBeLessThanOrEqual(44);
    expect(accountBox.x, "the pair is outside the left rail").toBeGreaterThan(
      railBox.x + railBox.width + 100,
    );

    // Nothing at the bottom of the shell may be covered by the floating pair.
    expect(overlaps(accountBox, healthBox), "Account must not overlap source health").toBe(false);
    expect(overlaps(settingsBox, healthBox), "Settings must not overlap source health").toBe(false);
    expect(accountBox.y + accountBox.height, "Account must clear the status bar").toBeLessThanOrEqual(
      footerBox.y,
    );
    expect(
      settingsBox.y + settingsBox.height,
      "Settings must clear the status bar",
    ).toBeLessThanOrEqual(footerBox.y);
  });
}

test("Tab reaches Account then Settings, and each control still opens its surface", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");

  const focusOrder: string[] = [];
  for (let step = 0; step < 24; step += 1) {
    await page.keyboard.press("Tab");
    const label = await page.evaluate(() => {
      const active = document.activeElement as HTMLElement | null;
      return active?.getAttribute("aria-label") ?? (active?.textContent ?? "").trim();
    });
    if (label) focusOrder.push(label);
    if (label === "Settings") break;
  }

  const accountAt = focusOrder.indexOf("Account");
  const settingsAt = focusOrder.indexOf("Settings");
  expect(accountAt, "Tab reaches Account").toBeGreaterThan(-1);
  expect(settingsAt, "Tab reaches Settings").toBeGreaterThan(-1);
  expect(settingsAt, "Account comes before Settings in the tab order").toBe(accountAt + 1);

  const cluster = page.getByRole("group", { name: "Account and settings" });
  const account = cluster.getByRole("button", { name: "Account", exact: true });
  const settings = cluster.getByRole("button", { name: "Settings", exact: true });

  await account.click();
  await expect(page.getByRole("heading", { name: "Your account" })).toBeVisible();
  await expect(account).toHaveAttribute("aria-pressed", "true");
  await expect(settings).toHaveAttribute("aria-pressed", "false");
  await expect(account).toHaveClass(/active/);

  await settings.click();
  await expect(page.getByRole("heading", { name: "Agent", exact: true })).toBeVisible();
  await expect(settings).toHaveAttribute("aria-pressed", "true");
  await expect(account).toHaveAttribute("aria-pressed", "false");
  await expect(settings).toHaveClass(/active/);
});
