import { expect, test } from "@playwright/test";

// Roadmap 111: role, tool, error, model, approval and cost are views of the same rows. The bar,
// the outline, in-session search and the hidden-row count must all agree on what is visible.

const open = async (page: import("@playwright/test").Page) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();
  return {
    bar: page.getByRole("group", { name: "Transcript filter" }),
    transcript: page.getByRole("region", { name: "Transcript" }),
  };
};

test("the role, model, approval and cost views share one visible-row rule", async ({ page }) => {
  const { bar, transcript } = await open(page);
  for (const name of ["yours", "model", "tools", "diffs", "errors", "approvals", "cost"]) {
    await expect(bar.getByRole("button", { name: new RegExp(`^${name}`) })).toBeVisible();
  }

  await bar.getByRole("button", { name: /^model/ }).click();
  await expect(transcript.getByText(/The ordering fix is right/).first()).toBeVisible();
  await expect(transcript.getByText("read_file")).toHaveCount(0);
  await expect(transcript.getByText(/Fix the resume test/)).toHaveCount(0);
  const outline = page.getByRole("navigation", { name: "Turn outline" });
  await expect(outline.locator(".outline-mark.model").first()).toBeVisible();
  await expect(outline.locator(".outline-mark.diff")).toHaveCount(0);

  await bar.getByRole("button", { name: /^cost/ }).click();
  // The usage row lives in a collapsed details element; presence is the evidence here.
  await expect(transcript.locator(".step-cost")).not.toHaveCount(0);
  await expect(transcript.getByText("read_file")).toHaveCount(0);
  await expect(transcript.locator(".turn-hidden").first()).toContainText(
    "hidden by the cost filter",
  );
});

test("in-session search narrows the outline and the count, and the view survives a reload", async ({
  page,
}) => {
  const { bar, transcript } = await open(page);
  const search = bar.getByRole("textbox", { name: "Search this transcript" });
  await search.fill("ordering");
  await expect(transcript.getByText(/The ordering fix is right/).first()).toBeVisible();
  await expect(transcript.getByText("read_file")).toHaveCount(0);
  const outline = page.getByRole("navigation", { name: "Turn outline" });
  await expect(outline.locator(".outline-mark.diff")).toHaveCount(0);
  await expect(transcript.locator(".turn-hidden").first()).toContainText("ordering");

  await search.fill("");
  await bar.getByRole("button", { name: /^approvals/ }).click();
  await expect(bar.getByRole("button", { name: /^approvals/ })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await page.reload();
  await page.getByRole("button", { name: "Session", exact: true }).click();
  await expect(
    page
      .getByRole("group", { name: "Transcript filter" })
      .getByRole("button", { name: /^approvals/ }),
  ).toHaveAttribute("aria-pressed", "true");
});
