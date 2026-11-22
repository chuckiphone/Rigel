import { expect, test } from "@playwright/test";

// Roadmap 66: one table shows the project ceiling, every role and every agent. The browser
// preview has no Tauri host, so the project file is an in-memory matrix; the intersection and the
// dispatcher refusal are covered by crates/rigel-core/tests/permissions.rs.
test("the matrix narrows a role with the project grant and cannot widen it", async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Permissions/ }).click();

  await expect(page.getByRole("heading", { name: "Project grant and role ceiling" })).toBeVisible();
  const table = page.getByRole("table", { name: "Permission matrix" });
  await expect(table).toBeVisible();

  const headers = await table.locator("thead th").allTextContents();
  const reviewer = headers.findIndex((header) => header.includes("Reviewer"));
  expect(reviewer).toBeGreaterThan(0);
  // The header includes the sticky Tool column; data cells do not.
  const reviewerCell = reviewer - 1;

  const runRow = table.getByRole("row", { name: /run_command/ });
  const projectBox = runRow.getByRole("checkbox", {
    name: "Grant run_command to the project",
  });
  await expect(projectBox).toBeChecked();
  await expect(runRow.locator("td").nth(reviewerCell)).toHaveAttribute(
    "data-allowed",
    "true",
  );

  // Removing a tool from the project removes it from every role. There is no role checkbox that
  // could add it back.
  await projectBox.uncheck();
  await expect(page.getByText("unsaved changes")).toBeVisible();
  await expect(runRow.locator("td").nth(reviewerCell)).toHaveAttribute(
    "data-allowed",
    "false",
  );
  await expect(
    runRow.locator("td").nth(reviewerCell).locator("span"),
  ).toHaveAttribute("title", "Narrowed by the project grant");

  const editorTool = table.getByRole("row", { name: /email_send/ });
  await editorTool
    .getByRole("checkbox", { name: "Grant email_send to the project" })
    .uncheck();
  await page.getByRole("button", { name: "Save permission matrix" }).click();
  await expect(page.getByText("unsaved changes")).toBeHidden();
  await expect(projectBox).not.toBeChecked();
  await expect(
    editorTool.getByRole("checkbox", { name: "Grant email_send to the project" }),
  ).not.toBeChecked();

  // The saved ceiling is the one the next turn reads; the role cells already reflect it.
  await expect(page.getByText("21 of 23 granted")).toBeVisible();
});

test("an agent override narrows one agent and cannot widen its role", async ({ page }) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Permissions/ }).click();

  const table = page.getByRole("table", { name: "Permission matrix" });
  const runRow = table.getByRole("row", { name: /run_command/ });
  const workerBox = runRow.getByRole("checkbox", {
    name: "Grant run_command to agent-preview-worker",
  });
  await expect(workerBox).toBeChecked();
  await workerBox.uncheck();
  await expect(page.getByText("unsaved changes")).toBeVisible();
  await expect(workerBox).not.toBeChecked();

  await page.getByRole("button", { name: "Save permission matrix" }).click();
  await expect(page.getByText("unsaved changes")).toBeHidden();
  await expect(workerBox).not.toBeChecked();

  // A reviewer agent does not get edit_file merely because a checkbox exists for it.
  const editRow = table.getByRole("row", { name: /edit_file/ });
  await expect(
    editRow.getByRole("checkbox", { name: "Grant edit_file to agent-preview-reviewer" }),
  ).toBeDisabled();
});
