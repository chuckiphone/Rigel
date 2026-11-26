import { expect, test } from "@playwright/test";

// Roadmap 116: the preview carries a shaped fake key, so the reporter has to redact it before it
// is shown or copied. The browser has no upload endpoint; Send is consent-gated and copies.
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

test("the crash reporter redacts, gates Send on consent and copies the exact preview", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .getByRole("group", { name: "Account and settings" })
    .getByRole("button", { name: "Settings", exact: true })
    .click();
  await page.getByRole("option", { name: /^Diagnostics/ }).click();

  const reporter = page.getByRole("region", { name: "Crash reporter" });
  await expect(reporter).toBeVisible();
  const preview = reporter.getByLabel("Redacted crash preview");
  const text = await preview.innerText();
  expect(text).toContain("Rigel crash report");
  expect(text).toContain("Bearer [redacted:secret]");
  expect(text).not.toContain("sk-xxxxxxxx");
  expect(text).toContain("panic in coordinator");

  const send = reporter.getByRole("button", { name: "Send report" });
  await expect(send).toBeDisabled();
  await reporter.getByRole("checkbox", { name: /I consent to sharing/ }).check();
  await expect(send).toBeEnabled();

  // Copy works without consent and writes the same bytes as the preview.
  await reporter.getByRole("checkbox", { name: /I consent to sharing/ }).uncheck();
  await reporter.getByRole("button", { name: "Copy report" }).click();
  const copied = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(copied).toBe(text);

  await reporter.getByRole("checkbox", { name: /I consent to sharing/ }).check();
  await send.click();
  await expect(reporter.getByRole("status")).toContainText("Nothing was uploaded");
  const sent = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(sent).toBe(text);
});
