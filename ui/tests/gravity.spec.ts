import { expect, test, type Locator } from "@playwright/test";

// The Gravity card is opt-in, and every part of the opt-in is visible: the switch, the test-ad
// switch, the shared endpoint and the labelled cards on the paper and in Session. Off means the
// slots are absent. Both cards show the advertiser's mark when a fill supplies one; Session's band
// is orange and its monogram remains the fallback.

async function expectArtwork(card: Locator, shaded = false) {
  const mark = card.locator("img");
  await expect(mark).toHaveCount(1);
  await expect(mark).toHaveAttribute("src", /^data:image\//);
  const painted = await card.evaluate((node) =>
    [node, ...node.querySelectorAll("*")]
      .flatMap((element) =>
        ["", "::before", "::after"].map(
          (pseudo) => getComputedStyle(element, pseudo || null).backgroundImage,
        ),
      )
      .filter((value) => value !== "none"),
  );
  expect(painted.some((value) => value.includes("url("))).toBe(false);
  if (shaded) {
    expect(painted.some((value) => value.includes("gradient"))).toBe(true);
    expect(painted.some((value) => value.includes("rgb(169, 74, 8)"))).toBe(true);
    expect(painted.some((value) => value.includes("rgb(91, 151, 64)"))).toBe(false);
  } else {
    expect(painted).toEqual([]);
  }
}

test("sponsored cards are opt-in, labelled and drawn from the preview ad", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("option", { name: /Sponsors/ }).click();

  await expect(page.getByRole("heading", { name: "Sponsored cards" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Shared endpoint" })).toBeVisible();
  await expect(page.getByText("https://rigel-25128.web.app/api/gravity-ad")).toBeVisible();

  const cardSwitch = page.getByRole("checkbox", { name: "Show sponsored cards in Rigel" });
  await expect(cardSwitch).not.toBeChecked();
  await expect(page.getByRole("checkbox", { name: /Show test ads/ })).toBeDisabled();

  await cardSwitch.check();
  await expect(page.getByRole("checkbox", { name: /Show test ads/ })).toBeChecked();

  await page.getByRole("button", { name: "Home", exact: true }).click();
  const homeCard = page.getByRole("region", { name: "Sponsored" });
  await expect(homeCard).toBeVisible();
  await expect(homeCard.getByText("Advertisement", { exact: true })).toBeVisible();
  await expect(
    homeCard.getByRole("heading", { name: "Ship a database this afternoon" }),
  ).toBeVisible();
  await expect(homeCard.getByRole("button", { name: /Learn more/ })).toBeVisible();
  await expectArtwork(homeCard);

  await page.getByRole("button", { name: "Session", exact: true }).click();
  const sessionCard = page.getByRole("region", { name: "Sponsored" });
  await expect(sessionCard).toBeVisible();
  await expect(sessionCard.getByText("Ad", { exact: true })).toBeVisible();
  await expect(sessionCard.getByText("Ship a database this afternoon", { exact: true })).toBeVisible();
  await expect(sessionCard.getByText("Learn more", { exact: true })).toBeVisible();
  await expectArtwork(sessionCard, true);

  // Off again: both surfaces drop the slot instead of leaving an empty box.
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("option", { name: /Sponsors/ }).click();
  await cardSwitch.uncheck();
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await expect(page.getByRole("region", { name: "Sponsored" })).toHaveCount(0);
  await page.getByRole("button", { name: "Home", exact: true }).click();
  await expect(page.getByRole("region", { name: "Sponsored" })).toHaveCount(0);
});
