import { expect, test } from "@playwright/test";

// Roadmap 118: a provider failure stays in its turn row. The preview fixture supplies the status,
// the provider's own message and a request id; nothing here needs a live credential or a network.
const cases = [
  {
    code: "401",
    message: "401 invalid x-api-key",
    remedy: "OpenRouter key again",
    request: "req-preview-401",
  },
  {
    code: "429",
    message: "429 rate limit exceeded",
    remedy: "Wait 12 s",
    request: "req-preview-429",
  },
  {
    code: "500",
    message: "500 internal server error",
    remedy: "Wait a moment",
    request: "req-preview-500",
  },
] as const;

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

for (const { code, message, remedy, request } of cases) {
  test(`a ${code} shows the provider message, remedy, request id and a copyable diagnosis in its turn row`, async ({
    page,
  }) => {
    await page.addInitScript((status) => {
      window.localStorage.setItem("rigel.preview.providerError", status);
    }, code);
    await page.goto("/");
    await page.getByRole("button", { name: "Session", exact: true }).click();

    const turn = page.locator("article.turn", {
      hasText: "Inspect the provider error and report it.",
    });
    await expect(turn).toBeVisible();
    const panel = turn.getByRole("alert", { name: `Provider error ${code}` });
    await expect(panel).toBeVisible();
    await expect(panel).toHaveAttribute("data-status", code);
    await expect(panel).toContainText(message);
    await expect(panel).toContainText(remedy);
    await expect(panel).toContainText(request);

    await panel.getByRole("button", { name: "Copy diagnostics" }).click();
    const copied = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
    expect(copied).toContain(`provider error ${code}`);
    expect(copied).toContain(message);
    expect(copied).toContain(request);
    expect(copied).not.toContain("sk-");

    // The row is still the transcript row after a reload, not a toast or a replaced surface.
    await page.reload();
    await page.getByRole("button", { name: "Session", exact: true }).click();
    await expect(turn).toBeVisible();
    await expect(turn.locator(".provider-error")).toContainText(request);
  });
}
