import { expect, test } from "@playwright/test";

// Roadmap 126 / docs/design.md: the webview is verified at 320, 375, 414 and 768 px. A surface
// must not scroll sideways, and no clickable label may wrap onto a second line. The measurement
// uses the painted text rects, not an estimate from the label length.

const WIDTHS = [320, 375, 414, 768];

async function overflow(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const client = document.documentElement.clientWidth;
    const offenders = Array.from(document.querySelectorAll<HTMLElement>("body *"))
      .filter((element) => {
        const box = element.getBoundingClientRect();
        return box.width > 0 && box.right > client + 1;
      })
      .slice(0, 8)
      .map((element) => {
        const box = element.getBoundingClientRect();
        const name = String(element.className).slice(0, 50);
        const owner = element.closest("section, div");
        return `${element.tagName.toLowerCase()}.${name} right=${Math.round(box.right)} owner=${String(owner?.className).slice(0, 60)}`;
      });
    return { scroll: document.documentElement.scrollWidth, client, offenders };
  });
}

/** Labels whose text painted on more than one line, with their text for a useful failure. */
async function wrappedLabels(page: import("@playwright/test").Page) {
  return page.evaluate(() => {
    const wrapped: string[] = [];
    for (const element of document.querySelectorAll<HTMLElement>("a[href], button")) {
      if (element.getAttribute("aria-hidden") === "true") continue;
      // Long prose titles are content, not control labels; the contract is about control labels.
      if ((element.textContent ?? "").trim().length > 24) continue;
      const style = getComputedStyle(element);
      if (style.display === "none" || style.visibility === "hidden") continue;
      const box = element.getBoundingClientRect();
      if (box.width === 0 || box.height === 0) continue;
      const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
      let node: Node | null;
      while ((node = walker.nextNode()) !== null) {
        const text = (node.textContent ?? "").trim();
        if (!text) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        const lines = Array.from(range.getClientRects()).filter(
          (rect) => rect.width > 0 && rect.height > 0,
        );
        if (lines.length > 1) wrapped.push(`${element.tagName.toLowerCase()}: ${text.slice(0, 60)}`);
      }
    }
    return wrapped;
  });
}

for (const width of WIDTHS) {
  test(`${width}px keeps Home and Session inside the viewport with single-line labels`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const surface of ["Home", "Session"] as const) {
      await page.goto("/");
      if (surface === "Session") {
        await page.getByRole("button", { name: "Session", exact: true }).click();
      }
      await expect(page.getByRole("main")).toBeVisible();

      const size = await overflow(page);
      expect(
        size.scroll,
        `${width}px ${surface}: document scrollWidth ${size.scroll} exceeds ${size.client}; ${size.offenders.join(", ")}`,
      ).toBeLessThanOrEqual(size.client);

      const wrapped = await wrappedLabels(page);
      expect(wrapped, `${width}px ${surface}: labels wrapped to two lines`).toEqual([]);
    }
  });
}
