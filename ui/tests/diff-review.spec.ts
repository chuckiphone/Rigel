import { expect, test } from "@playwright/test";

// Roadmap 132: the session's recorded hunks open in a review panel with both modes, per-hunk
// provenance, single-hunk acceptance and a verdict stored on the turn.

async function openReview(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await page.getByRole("button", { name: "Review diff" }).click();
  return page.getByRole("region", { name: "Diff review" });
}

test("hunks render in unified mode and toggle to split with a provenance rail", async ({ page }) => {
  const review = await openReview(page);
  await expect(review).toBeVisible();
  await expect(review.getByRole("group", { name: "Diff mode" }).getByRole("button", { name: "Unified" })).toHaveAttribute("aria-pressed", "true");
  await expect(review.getByRole("group", { name: "Diff mode" }).getByRole("button", { name: "Split" })).toHaveAttribute("aria-pressed", "false");
  await expect(review.getByLabel("Unified view for hunk 1")).toBeVisible();

  await review.getByRole("button", { name: "Split" }).click();
  await expect(review.getByLabel("Split view for hunk 1")).toBeVisible();
  await expect(review.getByLabel("Split view for hunk 2")).toBeVisible();

  await expect(review.getByLabel("Provenance for hunk 1")).toContainText("agent-preview-worker");
  await expect(review.getByLabel("Provenance for hunk 2")).toContainText("agent-preview-reviewer");
});

test("a hunk can be accepted alone and the named verdict is stored with the turn", async ({ page }) => {
  const review = await openReview(page);
  await review.getByRole("button", { name: "Accept hunk 2 alone" }).click();
  await expect(review.getByRole("button", { name: "Accepted hunk 2" })).toBeVisible();
  await expect(review).toContainText("1 of 2 hunk(s) accepted");

  await review.getByRole("textbox", { name: "Reviewer name" }).fill("reviewer@example.com");
  await review.getByRole("button", { name: "Save verdict with turn" }).click();
  await expect(review.getByRole("status")).toContainText(
    "Verdict stored with the turn by reviewer@example.com",
  );
});
