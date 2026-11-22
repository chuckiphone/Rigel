/**
 * The Session sponsored strip: one Gravity card above the composer.
 *
 * The card is opt-in. When sponsors are off, or Gravity returns no fill, this component renders
 * nothing, so no empty card or placeholder is left behind. The band, tile, button and depth are
 * CSS-only; the tile shows the advertiser's mark when Gravity supplies one, and its initials as
 * the fallback when it does not.
 */

import { useEffect, useRef, useState } from "react";

import { api, type GravityAd } from "./api";
import { adArtwork, adInitial, sponsorSession } from "./sponsor";

/** The host's ad request also carries the client user agent and OS, never a file or transcript. */
function gravityDevice() {
  return {
    ua: navigator.userAgent,
    ip: "",
    os: navigator.userAgent.includes("Windows") ? "Windows" : "",
  };
}

export default function CompanionSponsor({
  project,
  sessionId,
}: {
  project: string;
  sessionId: string;
}) {
  const [ad, setAd] = useState<GravityAd | null>(null);
  const [opening, setOpening] = useState(false);
  const [failed, setFailed] = useState(false);
  const cardRef = useRef<HTMLElement>(null);
  const impressed = useRef<string | null>(null);

  // Settings are read on every session entry, so turning sponsors off removes the strip. The
  // request itself is omitted when the switch is off.
  useEffect(() => {
    let live = true;
    impressed.current = null;
    setAd(null);
    setOpening(false);
    setFailed(false);
    void (async () => {
      try {
        const settings = await api.gravitySettings();
        if (!live || !settings.enabled) return;
        const ads = await api.gravityAd(
          sponsorSession(project, "session"),
          [
            {
              role: "user",
              content: "Show one sponsor for a developer working in a local-first coding session.",
            },
          ],
          gravityDevice(),
        );
        if (live) setAd(ads[0] ?? null);
      } catch {
        if (live) setAd(null);
      }
    })();
    return () => {
      live = false;
    };
  }, [project, sessionId]);

  // Count a view only when at least half the strip is visible, not merely mounted above the fold.
  useEffect(() => {
    const card = cardRef.current;
    if (!card || !ad?.impUrl || impressed.current === ad.impUrl) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (document.visibilityState !== "visible") return;
        if (!entries.some((entry) => entry.isIntersecting && entry.intersectionRatio >= 0.5)) return;
        if (impressed.current === ad.impUrl) return;
        impressed.current = ad.impUrl;
        void api
          .gravityImpression(ad.impUrl)
          .then(() => {
            card.dataset.impression = "recorded";
          })
          .catch(() => {
            card.dataset.impression = "failed";
          });
        observer.disconnect();
      },
      { threshold: 0.5 },
    );
    const observeVisible = () => {
      if (document.visibilityState !== "visible" || impressed.current === ad.impUrl) return;
      observer.unobserve(card);
      observer.observe(card);
    };
    observer.observe(card);
    document.addEventListener("visibilitychange", observeVisible);
    return () => {
      observer.disconnect();
      document.removeEventListener("visibilitychange", observeVisible);
    };
  }, [ad?.impUrl]);

  const open = async () => {
    if (!ad?.clickUrl || opening) return;
    setOpening(true);
    setFailed(false);
    try {
      await api.gravityClick(ad.clickUrl, ad.url, ad.test);
    } catch {
      setFailed(true);
    } finally {
      setOpening(false);
    }
  };

  if (!ad) return null;
  const art = adArtwork(ad);

  return (
    <div className="companion-dock">
      <section ref={cardRef} className="companion-card" aria-label="Sponsored" aria-busy={opening}>
        <a
          className="companion-ad"
          href={ad.test && ad.url ? ad.url : ad.clickUrl}
          rel="sponsored noopener noreferrer"
          data-gravity-ad
          aria-label={`Sponsored by ${ad.brandName || "an advertiser"}: ${ad.title || "offer"}`}
          onClick={(event) => {
            // The webview cannot follow Gravity's cross-origin link or load its pixel. Rust records
            // the click and opens the system browser, just as the rest of the app does.
            event.preventDefault();
            void open();
          }}
        >
          <span className="companion-ad-mark" aria-hidden="true" title={ad.brandName || undefined}>
            <span className="companion-ad-initial">{adInitial(ad.brandName)}</span>
            {art ? (
              <img
                key={art}
                className="companion-ad-logo"
                src={art}
                alt=""
                decoding="async"
                referrerPolicy="no-referrer"
                onError={(event) => {
                  event.currentTarget.hidden = true;
                }}
              />
            ) : null}
            <span className="companion-ad-badge" />
          </span>
          <span className="companion-ad-body">
            <span className="companion-ad-title-row">
              <span className="companion-ad-title" title={ad.title || undefined}>
                {ad.title || "From the wire"}
              </span>
              <span className="companion-ad-label">Ad</span>
            </span>
            {ad.adText ? (
              <span className="companion-ad-copy" title={ad.adText}>
                {ad.adText}
              </span>
            ) : null}
          </span>
          <span className="companion-ad-cta">
            {opening ? "Opening…" : ad.cta || "Learn more"}
          </span>
        </a>
        {failed ? (
          <p className="companion-card-fail" role="status">
            The sponsor's page could not be opened.
          </p>
        ) : null}
      </section>
    </div>
  );
}
