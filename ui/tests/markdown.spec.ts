import { expect, test } from "@playwright/test";

// Roadmap 112: a model answer with a script tag must be text, with a copied code block and a raw
// view of the exact source. No HTML is injected anywhere on this path.
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

test("the assistant answer renders script text, highlighted code and a raw toggle", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  const markdown = page.locator(".msg.assistant .markdown").filter({ hasText: "Cause" }).first();
  await expect(markdown).toBeVisible();
  await expect(markdown.locator("script")).toHaveCount(0);
  await expect(markdown).toContainText("<script>alert(1)</script>");

  const code = markdown.locator(".markdown-code");
  await expect(code).toBeVisible();
  await expect(code.locator(".markdown-language")).toHaveText("rust");
  expect(await code.locator("pre code span").count()).toBeGreaterThan(0);

  await code.getByRole("button", { name: "Copy code" }).click();
  const copied = await page.evaluate(() => (window as unknown as { __copied: string }).__copied);
  expect(copied).toBe("fn checkpoint() -> u64 {\n    let hash = tree_hash(); // the entry may lag the file\n    hash\n}");

  await markdown.getByRole("button", { name: "Raw" }).click();
  await expect(markdown.locator(".markdown-raw-body")).toBeVisible();
  await expect(markdown.locator(".markdown-raw-body")).toContainText("<script>alert(1)</script>");
  await expect(markdown.locator(".markdown-raw-body")).toContainText("```rust");
  await expect(code).toHaveCount(0);

  await markdown.getByRole("button", { name: "Rendered" }).click();
  await expect(code).toBeVisible();
});
