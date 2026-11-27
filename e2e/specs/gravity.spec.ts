import { mkdirSync } from "node:fs";
import path from "node:path";

import { $, browser, expect } from "@wdio/globals";
import {
  approveEveryCard,
  openRail,
  sendPrompt,
  startSession,
  waitForText,
} from "../lib/app";

/**
 * The Gravity card uses the real Tauri host and a real broker response. The runner supplies
 * `RIGEL_GRAVITY_ENDPOINT` and `RIGEL_GRAVITY_BROKER_URL` from the local harness broker
 * (http://127.0.0.1:5000 by default); the publisher key stays on that broker process. This spec
 * observes the Session card below the completed turn, the Rust impression receipt, the click
 * path (the runner suppresses the system browser, so no tab is left behind), the Home card, and
 * the no-fill/fallback branches.
 *
 * Start the broker with
 * `GRAVITY_TEST_FIXTURE=1 node --env-file=functions/.env scripts/gravity-test-broker.mjs`.
 * Set `RIGEL_GRAVITY_EXPECT_NO_FILL=1` for the no-fill pass, or `RIGEL_GRAVITY_NO_ART=1` for the
 * unsafe/missing-art fallback pass.
 */
const BROKER = process.env.RIGEL_GRAVITY_BROKER_URL ?? "http://127.0.0.1:5000";
const EVIDENCE = path.resolve(
  process.env.RIGEL_GRAVITY_EVIDENCE_DIR ?? path.join("tmp", "gravity-evidence"),
);
const NO_FILL = process.env.RIGEL_GRAVITY_EXPECT_NO_FILL === "1";
const NO_ART = process.env.RIGEL_GRAVITY_NO_ART === "1";
mkdirSync(EVIDENCE, { recursive: true });

type PublicCreative = {
  title: string;
  brand: string;
  text: string;
  cta: string;
  click: string;
  impression: string;
  favicon?: string;
  brandImage?: string;
};

type BrokerState = {
  fill: boolean;
  cached: PublicCreative | null;
  fixture?: {
    session: PublicCreative;
    home: PublicCreative;
    website: PublicCreative;
    fallback: PublicCreative;
  } | null;
  noArt?: boolean;
};

async function brokerState(): Promise<BrokerState> {
  const response = await fetch(`${BROKER}/api/test-state`, { cache: "no-store" });
  expect(response.ok).toBe(true);
  return (await response.json()) as BrokerState;
}

/** Ask the real host for its Gravity status, which never contains a publisher key. */
async function gravitySettings(): Promise<Record<string, unknown>> {
  await browser.execute(() => {
    const holder = window as unknown as {
      __rigelSettings?: Record<string, unknown>;
      __TAURI_INTERNALS__?: {
        invoke: (command: string) => Promise<Record<string, unknown>>;
      };
    };
    holder.__rigelSettings = { pending: true };
    const internals = holder.__TAURI_INTERNALS__;
    if (!internals) {
      holder.__rigelSettings = { error: "no Tauri internals" };
      return;
    }
    internals
      .invoke("gravity_settings")
      .then((value) => {
        holder.__rigelSettings = value;
      })
      .catch((error) => {
        holder.__rigelSettings = { error: String(error) };
      });
  });

  let settings: Record<string, unknown> | undefined;
  await browser.waitUntil(
    async () => {
      settings = (await browser.execute(
        () => (window as unknown as { __rigelSettings?: Record<string, unknown> }).__rigelSettings,
      )) as Record<string, unknown> | undefined;
      return Boolean(settings && !settings.pending);
    },
    { timeout: 10_000, interval: 200, timeoutMsg: "gravity_settings did not answer" },
  );
  return settings as Record<string, unknown>;
}

/** Browser-side probe: CSP breaches, navigation attempts, and uncaught errors are evidence. */
async function installProbe() {
  await browser.execute(() => {
    const holder = window as unknown as {
      __rigelGravityProbe?: {
        violations: Array<Record<string, unknown>>;
        errors: string[];
        navigations: string[];
      };
      navigation?: {
        addEventListener: (name: string, fn: (event: { destination?: { url?: string } }) => void) => void;
      };
    };
    const probe: {
      violations: Array<Record<string, unknown>>;
      errors: string[];
      navigations: string[];
    } = { violations: [], errors: [], navigations: [] };
    holder.__rigelGravityProbe = probe;
    document.addEventListener("securitypolicyviolation", (event) => {
      probe.violations.push({
        directive: event.violatedDirective,
        blocked: event.blockedURI,
        source: event.sourceFile,
      });
    });
    window.addEventListener("error", (event) => probe.errors.push(String(event.message)));
    window.addEventListener("unhandledrejection", (event) =>
      probe.errors.push(String((event as PromiseRejectionEvent).reason)),
    );
    holder.navigation?.addEventListener("navigate", (event) => {
      const destination = event.destination?.url ?? "";
      // The app routes inside the hash with history.replaceState (#sessions, #settings/...).
      // Those are same-document navigations, not the ad opening a page: a click that leaves the
      // document is the only navigation this probe is looking for.
      const here = window.location.href.split("#")[0];
      const there = destination.split("#")[0];
      if (here !== there) probe.navigations.push(destination);
    });
  });
}

async function probe() {
  return (await browser.execute(
    () =>
      (
        window as unknown as {
          __rigelGravityProbe?: {
            violations: Array<Record<string, unknown>>;
            errors: string[];
            navigations: string[];
          };
        }
      ).__rigelGravityProbe ?? { violations: [], errors: [], navigations: [] },
  )) as { violations: Array<Record<string, unknown>>; errors: string[]; navigations: string[] };
}

async function cardText(card: { getText: () => Promise<string> }): Promise<string> {
  return (await card.getText()).replace(/\s+/g, " ").trim();
}

async function positionBelowDone() {
  return (await browser.execute(() => {
    const node = document.querySelector<HTMLElement>('section[aria-label="Sponsored"]');
    const done = Array.from(document.querySelectorAll<HTMLElement>("p,span,div"))
      .filter((element) => element.children.length === 0 && element.textContent?.trim() === "Done.")
      .pop();
    return {
      cardTop: node?.getBoundingClientRect().top ?? null,
      doneBottom: done?.getBoundingClientRect().bottom ?? null,
    };
  })) as { cardTop: number | null; doneBottom: number | null };
}

/** Painted backdrops inside the sponsored region, including pseudo-elements. */
async function sponsorPaint(): Promise<{ backdrops: string[]; imageUrls: string[] }> {
  return (await browser.execute(() => {
    const card = document.querySelector<HTMLElement>('section[aria-label="Sponsored"]');
    if (!card) return { backdrops: ["sponsored region missing"], imageUrls: [] };
    const backdrops: string[] = [];
    const imageUrls: string[] = [];
    for (const element of [card, ...card.querySelectorAll<HTMLElement>("*")]) {
      for (const pseudo of ["", "::before", "::after"]) {
        const value = getComputedStyle(element, pseudo || null).backgroundImage;
        if (!value || value === "none") continue;
        const painted = `${element.tagName.toLowerCase()}${pseudo}: ${value}`;
        backdrops.push(painted);
        if (value.includes("url(")) imageUrls.push(painted);
      }
    }
    return { backdrops, imageUrls };
  })) as { backdrops: string[]; imageUrls: string[] };
}

/**
 * What is actually painted in the sponsored card. Natural dimensions prove the remote image
 * decoded; the bounding rects prove it occupies the tile, and the initial is still present behind
 * it as the fallback. The aggregate overflow check catches fallback layout breakage.
 */
async function artworkState(): Promise<{
  card: boolean;
  count: number;
  src: string;
  currentSrc: string;
  complete: boolean;
  naturalWidth: number;
  naturalHeight: number;
  hidden: boolean | null;
  width: number;
  height: number;
  tileWidth: number;
  tileHeight: number;
  initial: string;
  cardWidth: number;
  cardHeight: number;
  overflowX: boolean;
  background: string;
}> {
  return (await browser.execute(() => {
    const card = document.querySelector<HTMLElement>('section[aria-label="Sponsored"]');
    if (!card) {
      return {
        card: false,
        count: 0,
        src: "",
        currentSrc: "",
        complete: false,
        naturalWidth: 0,
        naturalHeight: 0,
        hidden: null,
        width: 0,
        height: 0,
        tileWidth: 0,
        tileHeight: 0,
        initial: "",
        cardWidth: 0,
        cardHeight: 0,
        overflowX: false,
        background: "",
      };
    }
    const image = card.querySelector<HTMLImageElement>("img");
    const tile = card.querySelector<HTMLElement>(".companion-ad-mark, .paper-sponsor-mark");
    const initial = card.querySelector<HTMLElement>(".companion-ad-initial, .paper-sponsor-mark > span");
    const imageRect = image?.getBoundingClientRect();
    const tileRect = tile?.getBoundingClientRect();
    const cardRect = card.getBoundingClientRect();
    const surface = card.querySelector<HTMLElement>(".companion-ad") ?? card;
    return {
      card: true,
      count: card.querySelectorAll("img").length,
      src: image?.getAttribute("src") ?? "",
      currentSrc: image?.currentSrc ?? "",
      complete: image?.complete ?? false,
      naturalWidth: image?.naturalWidth ?? 0,
      naturalHeight: image?.naturalHeight ?? 0,
      hidden: image?.hidden ?? null,
      width: imageRect?.width ?? 0,
      height: imageRect?.height ?? 0,
      tileWidth: tileRect?.width ?? 0,
      tileHeight: tileRect?.height ?? 0,
      initial: initial?.textContent?.trim() ?? "",
      cardWidth: cardRect.width,
      cardHeight: cardRect.height,
      overflowX: card.scrollWidth > card.clientWidth + 1,
      background: getComputedStyle(surface).backgroundImage,
    };
  })) as {
    card: boolean;
    count: number;
    src: string;
    currentSrc: string;
    complete: boolean;
    naturalWidth: number;
    naturalHeight: number;
    hidden: boolean | null;
    width: number;
    height: number;
    tileWidth: number;
    tileHeight: number;
    initial: string;
    cardWidth: number;
    cardHeight: number;
    overflowX: boolean;
    background: string;
  };
}

async function scrollSponsoredIntoView() {
  await browser.execute(() => {
    document
      .querySelector('section[aria-label="Sponsored"]')
      ?.scrollIntoView({ block: "center" });
  });
}

async function waitForArtwork(expectedSrc: string) {
  let state = await artworkState();
  await browser.waitUntil(
    async () => {
      state = await artworkState();
      return (
        state.card &&
        state.count === 1 &&
        state.src === expectedSrc &&
        state.currentSrc === expectedSrc &&
        state.complete &&
        state.naturalWidth > 0 &&
        state.naturalHeight > 0 &&
        state.hidden === false &&
        state.width > 0 &&
        state.height > 0 &&
        !state.overflowX
      );
    },
    {
      timeout: 30_000,
      interval: 250,
      timeoutMsg: `the advertiser image ${expectedSrc} never painted in the sponsored card`,
    },
  );
  return state;
}

async function waitForFallback() {
  let state = await artworkState();
  await browser.waitUntil(
    async () => {
      state = await artworkState();
      return (
        state.card &&
        state.count === 0 &&
        /^[A-Z✦]{1,2}$/.test(state.initial) &&
        state.tileWidth >= 10 &&
        state.tileHeight >= 10 &&
        state.width === 0 &&
        state.height === 0 &&
        !state.overflowX
      );
    },
    {
      timeout: 30_000,
      interval: 250,
      timeoutMsg: "the unsafe/missing artwork did not fall back to a clean initial tile",
    },
  );
  return state;
}

describe("Gravity end to end", () => {
  it(
    NO_FILL
      ? "renders no card on no fill"
      : NO_ART
        ? "falls back cleanly when artwork is unsafe or missing"
        : "renders and clicks the sponsored card with real advertiser artwork",
    async () => {
      await (await $('nav[aria-label="Primary"]')).waitForDisplayed({ timeout: 40_000 });
      await installProbe();

      await startSession();
      await sendPrompt("Make the approved change.");
      await approveEveryCard();
      await waitForText("Done.");

      const settings = await gravitySettings();
      expect(settings).toMatchObject({
        enabled: true,
        test_ads: true,
        placement_id: "rigel-sponsor",
      });
      expect(settings.endpoint).toBe(process.env.RIGEL_GRAVITY_ENDPOINT);
      expect(settings).not.toHaveProperty("key_present");

      if (NO_FILL) {
        await browser.pause(2_500);
        const sessionCard = await $('section[aria-label="Sponsored"]');
        expect(await sessionCard.isExisting()).toBe(false);
        const sessionText = await (await $("body")).getText();
        expect(sessionText).not.toMatch(/sample sponsor|from the wire|ship a database/i);
        await browser.saveScreenshot(path.join(EVIDENCE, "session-no-fill.png"));

        await openRail("Home");
        await waitForText("STANDING NOTICE");
        await browser.pause(2_500);
        expect(await (await $('section[aria-label="Sponsored"]')).isExisting()).toBe(false);
        const homeText = await (await $("body")).getText();
        expect(homeText).not.toMatch(/sample sponsor|from the wire|ship a database/i);
        await browser.saveScreenshot(path.join(EVIDENCE, "home-no-fill.png"));

        const observed = await probe();
        console.log(`no-fill probe: ${JSON.stringify(observed)}`);
        expect(observed.violations).toEqual([]);
        expect(observed.errors).toEqual([]);
        return;
      }

      if (NO_ART) {
        const sessionCard = await $('section[aria-label="Sponsored"]');
        await sessionCard.waitForDisplayed({ timeout: 30_000 });
        await scrollSponsoredIntoView();
        const sessionArt = await waitForFallback();
        console.log(`session fallback: ${JSON.stringify(sessionArt)}`);
        expect(sessionArt.background).toContain("rgb(169, 74, 8)");
        await browser.saveScreenshot(path.join(EVIDENCE, "session-fallback.png"));

        await openRail("Home");
        const homeCard = await $('section[aria-label="Sponsored"]');
        await homeCard.waitForDisplayed({ timeout: 60_000 });
        await scrollSponsoredIntoView();
        const homeArt = await waitForFallback();
        console.log(`home fallback: ${JSON.stringify(homeArt)}`);
        await browser.saveScreenshot(path.join(EVIDENCE, "home-fallback.png"));

        const observed = await probe();
        console.log(`fallback probe: ${JSON.stringify(observed)}`);
        expect(observed.violations).toEqual([]);
        expect(observed.errors).toEqual([]);
        return;
      }

      const state = await brokerState();
      expect(state.fill).toBe(true);
      const fixture = state.fixture ?? null;
      const sessionArtUrl = fixture?.session.favicon || state.cached?.favicon || "";
      const homeArtUrl = fixture?.home.brandImage || fixture?.home.favicon || state.cached?.favicon || "";
      expect(sessionArtUrl).toMatch(/^https:\/\//);
      expect(homeArtUrl).toMatch(/^https:\/\//);
      console.log(
        `broker artwork: session=${sessionArtUrl} home=${homeArtUrl} cached=${JSON.stringify(state.cached)}`,
      );

      const sessionCard = await $('section[aria-label="Sponsored"]');
      await sessionCard.waitForDisplayed({ timeout: 30_000 });
      await scrollSponsoredIntoView();
      const sessionArt = await waitForArtwork(sessionArtUrl);
      console.log(`session artwork: ${JSON.stringify(sessionArt)}`);
      expect(sessionArt.background).toContain("rgb(169, 74, 8)");
      const sessionPaint = await sponsorPaint();
      expect(sessionPaint.backdrops.some((value) => value.includes("linear-gradient"))).toBe(true);
      expect(sessionPaint.imageUrls).toEqual([]);
      const sessionCopy = await cardText(sessionCard);
      console.log(`session card: ${sessionCopy}`);
      expect(sessionCopy.toLowerCase()).toContain(
        (fixture?.session.title || state.cached?.title || "").toLowerCase(),
      );
      await browser.saveScreenshot(path.join(EVIDENCE, "session-neon.png"));

      const placement = await positionBelowDone();
      console.log(`session placement: ${JSON.stringify(placement)}`);
      expect(placement.cardTop).not.toBeNull();
      expect(placement.doneBottom).not.toBeNull();
      expect(placement.cardTop!).toBeGreaterThanOrEqual(placement.doneBottom!);

      await browser.waitUntil(
        async () =>
          (await browser.execute(() => {
            const card = document.querySelector<HTMLElement>('section[aria-label="Sponsored"]');
            return card?.dataset.impression === "recorded";
          })) === true,
        {
          timeout: 20_000,
          interval: 250,
          timeoutMsg: "the Session card never recorded its impression through Rust",
        },
      );
      console.log("session impression: recorded through Rust");

      const beforeUrl = await browser.getUrl();
      const adLink = await sessionCard.$("[data-gravity-ad]");
      await adLink.click();
      await browser.pause(500);
      await browser.waitUntil(
        async () => (await sessionCard.getAttribute("aria-busy")) === "false",
        {
          timeout: 30_000,
          interval: 250,
          timeoutMsg: "the sponsored click did not finish",
        },
      );
      expect(await browser.getUrl()).toBe(beforeUrl);
      expect(await sessionCard.getText()).not.toMatch(/could not be opened/i);
      console.log("session click: recorded with the system browser suppressed");

      const afterClick = await probe();
      console.log(`session click probe: ${JSON.stringify(afterClick)}`);
      expect(afterClick.violations).toEqual([]);
      expect(afterClick.errors).toEqual([]);
      // The ad must not open a page in this document; the app's own #sessions hash is filtered.
      expect(afterClick.navigations).toEqual([]);

      await openRail("Home");
      const homeCard = await $('section[aria-label="Sponsored"]');
      await homeCard.waitForDisplayed({ timeout: 60_000 });
      await scrollSponsoredIntoView();
      const homeArt = await waitForArtwork(homeArtUrl);
      console.log(`home artwork: ${JSON.stringify(homeArt)}`);
      const homeCopy = await cardText(homeCard);
      console.log(`home card: ${homeCopy}`);
      expect(homeCopy).toMatch(/advertisement/i);
      expect(homeCopy.toLowerCase()).toContain(
        (fixture?.home.title || state.cached?.title || "").toLowerCase(),
      );
      await browser.saveScreenshot(path.join(EVIDENCE, "home-vercel.png"));

      const finalProbe = await probe();
      console.log(`final probe: ${JSON.stringify(finalProbe)}`);
      expect(finalProbe.violations).toEqual([]);
      expect(finalProbe.errors).toEqual([]);
    },
  );
});
