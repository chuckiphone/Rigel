import { expect, test } from "@playwright/test";

// Roadmap 45: a conflict is resolved one hunk at a time. The browser preview has the same
// marker merge the desktop sees, so this checks the guidance, the path-derived test hint and
// the stored-reviewer wording without a database.
test("a conflicted merge needs every hunk, a reviewer and the test hint", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Diff", exact: true }).click();

  const conflicts = page.getByRole("region", { name: "Conflicts" });
  await conflicts.getByRole("button", { name: "see the three versions" }).click();
  await conflicts.getByRole("button", { name: "three-way merge" }).click();

  const editor = page.getByRole("region", { name: "Guided merge editor" });
  await expect(editor).toBeVisible();
  await expect(editor.getByRole("article", { name: "Conflict 1 of 2" })).toBeVisible();
  await expect(editor.getByRole("article", { name: "Conflict 2 of 2" })).toBeVisible();
  await expect(editor.getByText("There is no accept-all", { exact: false })).toBeVisible();
  await expect(editor.getByText("npm --prefix ui run test:e2e", { exact: true })).toBeVisible();

  const write = page.getByRole("button", { name: "write reviewed merge" });
  await expect(write).toBeDisabled();
  await expect(page.getByText("Decide conflict hunk 1 before writing.")).toBeVisible();

  await editor.getByRole("button", { name: "accept ours" }).first().click();
  await expect(write).toBeDisabled();
  await expect(page.getByText("Decide conflict hunk 2 before writing.")).toBeVisible();

  await editor.getByRole("button", { name: "accept theirs" }).nth(1).click();
  await expect(write).toBeDisabled();
  await expect(page.getByText("Name the reviewer before writing the merge.")).toBeVisible();

  await editor.getByRole("textbox", { name: "Reviewer" }).fill("Ada Reviewer");
  await expect(write).toBeDisabled();
  await expect(page.getByText("Confirm the test hint before writing the merge.")).toBeVisible();

  await editor.getByRole("checkbox", { name: "I ran this check or a more focused one." }).check();
  await expect(write).toBeEnabled();
  await write.click();

  await expect(page.getByText("2 conflict hunks decided", { exact: false })).toBeVisible();
  await expect(page.getByText("accepted by Ada Reviewer", { exact: false })).toBeVisible();
});