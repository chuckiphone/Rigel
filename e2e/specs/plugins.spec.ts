import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

import { $, browser, expect } from "@wdio/globals";
import { openSettingsPage, waitForText } from "../lib/app";

/**
 * The Settings → Plugins marketplace, driven through the production window.
 *
 * No command is stubbed: the page calls the same `plugins_*` Tauri commands the CLI uses, and
 * this spec reads the resulting store over that same IPC bridge. Install writes a receipt,
 * disable changes the next turn's `plugin_call` availability, and a manifest changed after
 * install is reported as broken and can still be removed.
 */

type PluginRow = {
  name: string;
  version: string;
  description: string;
  enabled: boolean;
  tools: string[];
  capabilities: string[];
  source: string | null;
  license: string | null;
  installed_at: number | null;
};

type PluginsState = {
  plugins: PluginRow[];
  broken: { name: string; error: string }[];
  enabled_tools: string[];
};

type CatalogCard = {
  name: string;
  text: string;
  tier: string;
  button: string;
  disabled: boolean;
};

const INDEX = process.env.RIGEL_INDEX_DIR;
if (!INDEX) throw new Error("RIGEL_INDEX_DIR is required for the plugin window spec");
const PLUGIN_DIR = path.join(INDEX, "plugins", "rigel-hello");
const MANIFEST = path.join(PLUGIN_DIR, "plugin.json");
const STATE = path.join(INDEX, "plugins", "state.json");
const ENTRY = '[data-testid="plugin-card"][data-plugin="rigel-hello"]';
const INSTALLED = '[data-testid="plugin-installed"][data-plugin="rigel-hello"]';

async function invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  return (await browser.execute(
    (cmd: string, payload: Record<string, unknown>) => {
      const bridge = (
        window as unknown as {
          __TAURI_INTERNALS__?: {
            invoke: (name: string, values?: Record<string, unknown>) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      if (!bridge) throw new Error("the Tauri IPC bridge is not present");
      return bridge.invoke(cmd, payload);
    },
    command,
    args,
  )) as T;
}

async function state(): Promise<PluginsState> {
  return invoke<PluginsState>("plugins_list");
}

async function card(): Promise<CatalogCard | null> {
  return (await browser.execute(() => {
    const element = document.querySelector('[data-testid="plugin-card"][data-plugin="rigel-hello"]');
    if (!element) return null;
    const button = element.querySelector<HTMLButtonElement>('[data-testid="plugin-install"]');
    const tier = element.querySelector(".plugins-tier")?.textContent?.trim() ?? "";
    return {
      name: element.getAttribute("data-plugin") ?? "",
      text: (element.textContent ?? "").replace(/\s+/g, " ").trim(),
      tier,
      button: button?.textContent?.trim() ?? "",
      disabled: button?.disabled ?? false,
    };
  })) as CatalogCard | null;
}

async function cardCount(): Promise<number> {
  return (await browser.execute(
    () => document.querySelectorAll('[data-testid="plugin-card"]').length,
  )) as number;
}

async function waitForCards(count: number) {
  await browser.waitUntil(async () => (await cardCount()) === count, {
    timeout: 15_000,
    interval: 100,
    timeoutMsg: `expected ${count} catalogue cards, saw ${await cardCount()}`,
  });
}

async function waitForCard(name: string) {
  const element = await $(`[data-testid="plugin-card"][data-plugin="${name}"]`);
  await element.waitForDisplayed({ timeout: 15_000 });
}

async function waitForCardsAtLeast(count: number) {
  await browser.waitUntil(async () => (await cardCount()) >= count, {
    timeout: 15_000,
    interval: 100,
    timeoutMsg: `expected at least ${count} catalogue cards, saw ${await cardCount()}`,
  });
}

async function clickChip(group: string, label: string) {
  const clicked = (await browser.execute(
    (groupLabel: string, text: string) => {
      const scope = document.querySelector(`[aria-label="${groupLabel}"]`);
      const button = Array.from(scope?.querySelectorAll("button") ?? []).find((candidate) =>
        (candidate.textContent ?? "").trim().includes(text),
      );
      button?.click();
      return Boolean(button);
    },
    group,
    label,
  )) as boolean;
  expect(clicked).toBe(true);
}

async function search(text: string) {
  const input = await $('input[aria-label="Search the plugin catalogue"]');
  if (text) {
    await input.setValue(text);
  } else {
    await input.clearValue();
  }
}

async function clearFilters() {
  const clear = await $('[data-testid="plugins-clear"]');
  await clear.waitForClickable({ timeout: 10_000 });
  await clear.click();
}

describe("the plugin marketplace", () => {
  before(async () => {
    await waitForText("e2e-project");
    await openSettingsPage("Plugins");
    await waitForCard("rigel-hello");
    await waitForCardsAtLeast(100);
    await waitForText("Rigel Hello");
  });

  it("renders the curated catalogue with tier, category, licence and capability chips", async () => {
    await waitForCardsAtLeast(100);
    const hello = await card();
    expect(hello).not.toBeNull();
    expect(hello?.tier).toBe("official");
    expect(hello?.text).toContain("plugin");
    expect(hello?.text).toContain("Examples");
    expect(hello?.text).toContain("MIT OR Apache-2.0");
    expect(hello?.text).toContain("filesystem read: .");
    expect(hello?.button).toBe("Install plugin");

    const evalCard = (await browser.execute(() => {
      const element = document.querySelector(
        '[data-testid="plugin-card"][data-plugin="skillevaluator"]',
      );
      return (element?.textContent ?? "").replace(/\s+/g, " ").trim();
    })) as string;
    expect(evalCard).toContain("NVIDIA SkillEvaluator");
    expect(evalCard).toContain("verified");
    expect(evalCard).toContain("Quality");
    expect(evalCard).toContain("Apache-2.0");
    expect(evalCard).toContain("process");

    const superpowers = (await browser.execute(() => {
      const element = document.querySelector(
        '[data-testid="plugin-card"][data-plugin="superpowers"]',
      );
      return (element?.textContent ?? "").replace(/\s+/g, " ").trim();
    })) as string;
    expect(superpowers).toContain("Superpowers");
    expect(superpowers).toContain("Workflow");
    expect(superpowers).toContain("Install skill");
  });

  it("narrows by search, tier and category", async () => {
    await search("rigel-hello");
    await waitForCards(1);
    expect((await card())?.name).toBe("rigel-hello");

    await clearFilters();
    await waitForCard("rigel-hello");
    await waitForCardsAtLeast(100);

    await clickChip("Catalogue tier", "official");
    await waitForCards(2);
    expect((await card())?.tier).toBe("official");

    await clearFilters();
    await waitForCard("rigel-hello");
    await waitForCardsAtLeast(100);

    await clickChip("Catalogue category", "Examples");
    await waitForCards(1);
    expect((await card())?.text).toContain("Examples");

    await clearFilters();
    await waitForCard("rigel-hello");
    await waitForCardsAtLeast(100);
  });

  it("installs the bundled plugin, writes a receipt and shows its tools and capabilities", async () => {
    const install = await $(
      `${ENTRY} [data-testid="plugin-install"]`,
    );
    await install.waitForClickable({ timeout: 10_000 });
    await install.click();

    const row = await $(INSTALLED);
    await row.waitForDisplayed({ timeout: 20_000 });
    await expect(row).toHaveText(expect.stringContaining("rigel-hello.hello"));
    await expect(row).toHaveText(expect.stringContaining("capabilities: read: ."));
    await expect(row).toHaveText(expect.stringContaining("bundled:rigel-hello"));
    expect((await card())?.button).toBe("Installed");

    const installed = await state();
    expect(installed.plugins).toHaveLength(1);
    expect(installed.plugins[0].enabled).toBe(true);
    expect(installed.plugins[0].tools).toEqual(["rigel-hello.hello", "rigel-hello.read_note"]);
    expect(installed.plugins[0].capabilities).toContain("read: .");
    expect(installed.plugins[0].source).toBe("bundled:rigel-hello");
    expect(installed.plugins[0].installed_at).toBeGreaterThan(0);
    expect(installed.enabled_tools).toContain("rigel-hello.hello");

    const receipt = JSON.parse(readFileSync(STATE, "utf8")) as {
      receipts?: Record<string, { source?: string; sha256?: string }>;
    };
    expect(receipt.receipts?.["rigel-hello"]?.source).toBe("bundled:rigel-hello");
    expect(receipt.receipts?.["rigel-hello"]?.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("disables and re-enables the plugin so its tools leave and return to the agent", async () => {
    const row = await $(INSTALLED);
    const toggle = await row.$('[data-testid="plugin-toggle"]');
    await toggle.click();

    await browser.waitUntil(
      async () => (await row.getText()).includes("Enable"),
      { timeout: 15_000, interval: 100, timeoutMsg: "the row never offered Enable" },
    );
    let current = await state();
    expect(current.plugins[0].enabled).toBe(false);
    expect(current.enabled_tools).toEqual([]);

    await toggle.click();
    await browser.waitUntil(
      async () => (await row.getText()).includes("Disable"),
      { timeout: 15_000, interval: 100, timeoutMsg: "the row never offered Disable again" },
    );
    current = await state();
    expect(current.plugins[0].enabled).toBe(true);
    expect(current.enabled_tools).toContain("rigel-hello.read_note");
  });

  it("surfaces a backend error and reports a tampered install as broken, not healthy", async () => {
    const manifest = JSON.parse(readFileSync(MANIFEST, "utf8")) as {
      capabilities?: Record<string, unknown>;
    };
    manifest.capabilities = { filesystem: { read: ["/"] } };
    writeFileSync(MANIFEST, JSON.stringify(manifest, null, 2));

    // The page still holds the healthy row. Its next write must show the backend's refusal,
    // not swallow it or leave the toggle looking successful.
    const row = await $(INSTALLED);
    await (await row.$('[data-testid="plugin-toggle"]')).click();
    const error = await $('[data-testid="plugins-error"]');
    await error.waitForDisplayed({ timeout: 15_000 });
    await expect(error).toHaveText(expect.stringContaining("changed since install"));
    await expect(error).toHaveText(expect.stringContaining("broken"));

    await (await $('[data-testid="plugins-refresh"]')).click();
    const broken = await $('[data-testid="plugins-broken"]');
    await broken.waitForDisplayed({ timeout: 15_000 });
    await expect(broken).toHaveText(expect.stringContaining("rigel-hello"));

    await browser.waitUntil(
      async () => !(await $(INSTALLED).isExisting()),
      { timeout: 15_000, interval: 100, timeoutMsg: "the broken plugin was still listed as healthy" },
    );
    const current = await state();
    expect(current.plugins).toEqual([]);
    expect(current.enabled_tools).toEqual([]);
    expect(current.broken).toHaveLength(1);
    expect(current.broken[0].error).toContain("changed since install");
    expect((await card())?.button).toBe("Broken — remove below");
    expect((await card())?.disabled).toBe(true);
  });

  it("removes a broken install and allows a clean reinstall, then removes the healthy tree", async () => {
    await (await $('[data-testid="plugin-broken-remove"]')).click();
    await browser.waitUntil(
      async () => !(await $('[data-testid="plugins-broken"]').isExisting()),
      { timeout: 15_000, interval: 100, timeoutMsg: "the broken section stayed after Remove" },
    );
    expect(existsSync(PLUGIN_DIR)).toBe(false);
    const receipt = JSON.parse(readFileSync(STATE, "utf8")) as {
      receipts?: Record<string, unknown>;
    };
    expect(receipt.receipts?.["rigel-hello"]).toBeUndefined();
    expect((await state()).plugins).toEqual([]);

    const install = await $(`${ENTRY} [data-testid="plugin-install"]`);
    await install.waitForClickable({ timeout: 10_000 });
    await install.click();
    const row = await $(INSTALLED);
    await row.waitForDisplayed({ timeout: 20_000 });

    await (await row.$('[data-testid="plugin-remove"]')).click();
    await browser.waitUntil(
      async () => !(await $(INSTALLED).isExisting()),
      { timeout: 15_000, interval: 100, timeoutMsg: "the installed row stayed after Remove" },
    );
    expect(existsSync(PLUGIN_DIR)).toBe(false);
    const final = await state();
    expect(final.plugins).toEqual([]);
    expect(final.broken).toEqual([]);
    expect(final.enabled_tools).toEqual([]);
    expect((await card())?.button).toBe("Install plugin");
  });
});
