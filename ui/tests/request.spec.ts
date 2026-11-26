import { expect, test } from "@playwright/test";

// Roadmap 119: the inspector shows one struct, drops credential values and copies that same struct.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        async writeText(text: string) {
          (window as unknown as { __copied: string }).__copied = text;
        },
      },
    });
  });
});

test("the request inspector redacts headers and copies the exact struct", async ({ page }) => {
  await page.goto("/?request=1");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const panel = page.getByRole("region", { name: "Request inspector" });
  await expect(panel).toBeVisible();
  await expect(panel).toContainText("openrouter / fast → openrouter / openai/gpt-5-nano");
  await expect(panel).toContainText("120 in · 45 out · 20 cached · 185 total");
  await expect(panel).toContainText("820 ms model · 35 ms queue");
  await expect(panel.getByText("read_file")).toBeVisible();
  await expect(panel.getByText("/v1/chat/completions")).toBeVisible();
  await expect(panel.getByText("chat-42")).toBeVisible();

  const redacted = panel.locator(".request-headers .redacted");
  expect(await redacted.count()).toBeGreaterThanOrEqual(3);
  for (const value of await redacted.locator("dd").allTextContents()) {
    expect(value).not.toContain("sk-preview-secret");
  }
  expect(await panel.innerText()).not.toContain("sk-preview-secret");

  await panel.getByRole("button", { name: "Copy as JSON" }).click();
  await expect(panel.getByRole("status")).toContainText("Copied the exact struct shown here");
  const copied = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  const value = JSON.parse(copied);
  expect(value.route.resolved_model).toBe("openai/gpt-5-nano");
  expect(value.tokens.total).toBe(185);
  expect(value.timing).toEqual({ duration_ms: 820, queue_ms: 35 });
  expect(value.tool_schema.name).toBe("read_file");
  for (const header of [...value.request.headers, ...value.response.headers]) {
    if (header.redacted) expect(header.value).toBe("");
  }
  expect(copied).not.toContain("sk-preview-secret");
});
