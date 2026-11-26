import { expect, test } from "@playwright/test";

// Roadmap 138: every state has the same live-region contract, and retry really runs.
test("each async state announces itself and retry restarts the work", async ({ page }) => {
  await page.goto("/?async=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const kit = page.getByRole("region", { name: "Async state kit" });
  await expect(kit).toBeVisible();
  const choose = page.getByRole("group", { name: "Choose async state" });
  const live = kit.locator(".async-live");

  for (const kind of ["loading", "empty", "error", "retry", "success"]) {
    await choose.getByRole("button", { name: kind, exact: true }).click();
    await expect(live).toContainText(`${kind} state`);
    await expect(kit.locator(".async-state")).toHaveAttribute("data-state", kind);
    await expect(live).toHaveAttribute(
      "aria-live",
      kind === "error" || kind === "retry" ? "assertive" : "polite",
    );
  }

  await choose.getByRole("button", { name: "error", exact: true }).click();
  await kit.getByRole("button", { name: "Try again" }).click();
  await expect(live).toHaveText("loading state");
  await expect(kit.locator(".async-state")).toHaveAttribute("data-state", "loading");

  await choose.getByRole("button", { name: "success", exact: true }).click();
  await expect(live).toHaveText("success state · 1 tries");
});
