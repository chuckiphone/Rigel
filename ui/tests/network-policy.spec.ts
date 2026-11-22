import { expect, test } from "@playwright/test";

// Roadmap 61: the editor is the same policy table the core enforces. The browser preview has no
// Tauri commands or project file, so this covers the rendered controls, the ordered rule rows and
// the explicit preview-only answer from the explain button; the save refusal, the matcher and the
// request-path refusal are covered by the core and CLI tests.
test("the network editor edits the kill switch, ordered rules and disabled sessions", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Network/ }).click();

  await expect(page.getByRole("heading", { name: "Project network policy" })).toBeVisible();
  const killSwitch = page.getByRole("checkbox", { name: "Project network kill switch" });
  await expect(killSwitch).toBeChecked();
  await killSwitch.uncheck();
  await expect(page.getByText("unsaved changes")).toBeVisible();
  await page.getByRole("combobox", { name: "Default network decision" }).selectOption("deny");

  await expect(page.getByLabel("Egress window seconds")).toHaveValue("60");
  await page.getByLabel("Requests per origin").fill("2");
  await page.getByLabel("Requests per agent").fill("4");
  await expect(page.getByLabel("Pin DNS resolution")).toBeChecked();
  await expect(page.getByLabel("Allow private network addresses")).not.toBeChecked();
  await page.getByLabel("Allow private network addresses").check();

  await page.getByRole("button", { name: "+ rule" }).click();
  await page.getByRole("combobox", { name: "Rule 1 action" }).selectOption("allow");
  await page.getByRole("textbox", { name: "Rule 1 pattern" }).fill("https://example.com");
  await page.getByRole("textbox", { name: "Rule 1 note" }).fill("docs");
  await expect(page.getByRole("button", { name: "Remove rule 1" })).toBeVisible();

  await page.getByRole("textbox", { name: "Session id to disable" }).fill("blocked");
  await page.getByRole("button", { name: "disable", exact: true }).click();
  await expect(page.getByRole("button", { name: "blocked ×" })).toBeVisible();

  // The preview's networkSet returns the edited policy; there is no file to write. The same
  // button talks to `network_set` in the desktop app, where core validation refuses bad rows.
  await page.getByRole("button", { name: "Save network policy" }).click();
  await expect(page.getByText("unsaved changes")).toBeHidden();
  await expect(killSwitch).not.toBeChecked();
  await expect(page.getByLabel("Requests per origin")).toHaveValue("2");
  await expect(page.getByLabel("Allow private network addresses")).toBeChecked();
  await expect(page.getByRole("combobox", { name: "Rule 1 action" })).toHaveValue("allow");

  await page.getByRole("textbox", { name: "URL to explain" }).fill("https://example.com/page");
  await page.getByRole("button", { name: "explain" }).click();
  await expect(page.getByText(/browser preview has no core policy check/)).toBeVisible();
});
