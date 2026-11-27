import { expect, test, type Page } from "@playwright/test";

// Roadmap 122: the browser preview keeps upload bytes in memory, so these runs exercise the same
// drop, validation, progress and cancel path the shell drives — without writing to a project.

async function dropFile(page: Page, name: string, size: number, type: string) {
  const transfer = await page.evaluateHandle(
    ({ name, size, type }) => {
      const data = new DataTransfer();
      data.items.add(new File([new Uint8Array(size)], name, { type }));
      return data;
    },
    { name, size, type },
  );
  const target = page.getByRole("group", { name: "Composer controls" });
  await target.dispatchEvent("dragover", { dataTransfer: transfer });
  await target.dispatchEvent("drop", { dataTransfer: transfer });
}

test("an accepted drop uploads with real progress and attaches only after completion", async ({
  page,
}) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  await dropFile(page, "notes.txt", 8, "text/plain");
  await expect(page.getByRole("list", { name: "Attached files" })).toContainText("notes.txt");
  await expect(page.getByRole("progressbar", { name: /Uploading notes.txt/ })).toHaveCount(0);
  await expect(page.getByRole("alert")).toHaveCount(0);
});

test("refused type and size never attach and explain why", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  await dropFile(page, "malware.exe", 16, "application/x-msdownload");
  await expect(page.getByRole("alert")).toContainText("malware.exe: .exe files cannot be attached");
  await dropFile(page, "huge.txt", 26 * 1024 * 1024, "text/plain");
  await expect(page.getByRole("alert")).toContainText(/huge.txt: .*25 MiB limit/);
  await expect(page.getByRole("list", { name: "Attached files" })).toHaveCount(0);
});

test("cancel stops a real upload and leaves no attachment behind", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "Session", exact: true }).click();

  await dropFile(page, "big.txt", 16 * 1024 * 1024, "text/plain");
  const progress = page.getByRole("progressbar", { name: /Uploading big.txt/ });
  await expect(progress).toBeVisible();
  await expect
    .poll(async () => Number((await progress.getAttribute("value")) ?? 0), { timeout: 2_000 })
    .toBeGreaterThan(0);

  await page.getByRole("button", { name: "Cancel upload" }).click();
  await expect(progress).toHaveCount(0);
  await expect(page.getByRole("list", { name: "Attached files" })).toHaveCount(0);
});
