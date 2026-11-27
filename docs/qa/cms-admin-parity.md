# CMS admin brand parity (roadmap 139)

The Payload admin at `/admin` uses Rigel's ink and amber tokens from
`cms/src/app/(payload)/custom.scss`, the inlined `RigelMark` from
`cms/src/components/RigelBrand.tsx`, and the Hallmark stamp recorded in that stylesheet. It stays
inside the brand while giving authors two real ways out:

- **Public site** — `https://askrigel.com/`, from `RIGEL_PUBLIC_SITE` in `cms/src/lib/rigel.ts`.
- **Firebase console** — every entry in `RIGEL_CLOUD_LINKS` points at the configured project,
  defaulting to `rigel-25128` and following `RIGEL_FIREBASE_PROJECT_ID` when set.

Both link groups render after the collection nav and on the dashboard through
`cms/src/components/RigelCloud.tsx`; each opens in a new tab with `rel="noreferrer"`. No remote
image is requested for the mark.

## Evidence

`cms-admin-parity.png` in this directory is captured by
`cms/tests/e2e/admin.e2e.spec.ts` from a signed-in `/admin` session. The same spec asserts the
mark is visible and that the public-site and Firebase links carry the expected targets. The
token/stamp assertions and the link list are covered without a running CMS by
`cms/tests/int/rigel.int.spec.ts`.

To refresh the screenshot: seed a local admin (`npm --prefix cms run seed`), start the CMS
(`npm --prefix cms run dev`), then run `npx playwright test --config cms/playwright.config.ts
cms/tests/e2e/admin.e2e.spec.ts` from the repository root with the CMS listening on port 3000.
