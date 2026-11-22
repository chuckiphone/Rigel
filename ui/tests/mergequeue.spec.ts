import { expect, test } from "@playwright/test";

// Roadmap 46: one writer per path. The preview ships a held lane with one waiter so the
// renderer can show the invariant without a live worktree; the core test owns the exclusion.
test("the merge queue names the holder and keeps one writer per path", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Diff", exact: true }).click();

  const queue = page.getByRole("region", { name: "Merge queue" });
  await expect(queue).toBeVisible();
  await expect(
    queue.getByText("One writer per path. A second writer waits, names the holder", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(queue.getByText("crates/rigel-core/src/retry.rs", { exact: true })).toBeVisible();

  // The lane has exactly one holder, and the waiting row names that same writer.
  await expect(queue.getByText("writing", { exact: true })).toHaveCount(1);
  await expect(queue.getByText("1 waiting", { exact: true })).toBeVisible();
  await expect(queue.getByText("is waiting on", { exact: false })).toHaveCount(1);
  await expect(queue.getByText("writer a1f4c9d2 holds this path", { exact: false })).toBeVisible();
  await expect(
    queue.getByText("writer b7e30211 is waiting on writer a1f4c9d2", { exact: false }),
  ).toBeVisible();
  await expect(queue.getByText("position 1", { exact: false })).toBeVisible();
});
