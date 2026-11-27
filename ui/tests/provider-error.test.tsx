import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  ProviderErrorPanel,
  parseProviderError,
  providerErrorDiagnostics,
  providerStatus,
} from "../src/provider-error";

const body = (status: number, extra: Record<string, unknown> = {}) => ({
  status,
  provider: "OpenRouter",
  model: "anthropic/claude-sonnet-4.5",
  message: `provider returned ${status}: a message from the provider`,
  request_id: `req-${status}`,
  ...extra,
});

test("only 401, 429 and 5xx are provider errors", () => {
  assert.equal(providerStatus(body(401)), 401);
  assert.equal(providerStatus(body(429)), 429);
  assert.equal(providerStatus(body(503)), 503);
  assert.equal(providerStatus({ message: "provider returned 502: bad gateway" }), 502);
  assert.equal(providerStatus({ message: "http error: 401 invalid key" }), 401);
  assert.equal(providerStatus({ status_code: "500", message: "" }), 500);
  assert.equal(providerStatus({ message: "stopped after 8 steps" }), null);
  assert.equal(providerStatus({ message: "stopped after 500 steps" }), null);
  assert.equal(providerStatus({ status: 404, message: "not found" }), null);
  assert.equal(providerStatus({}), null);
});

test("the panel carries the provider's own message, request id and one 401 remedy", () => {
  const error = parseProviderError(body(401));
  assert.ok(error);
  assert.equal(error.status, 401);
  assert.equal(error.message, "provider returned 401: a message from the provider");
  assert.equal(error.requestId, "req-401");
  assert.match(error.remedy, /OpenRouter/);
  assert.match(error.remedy, /Settings → Keys/);
  assert.match(error.remedy, /set the OpenRouter key again/);

  const html = renderToStaticMarkup(<ProviderErrorPanel body={body(401)} turnId="t-1" />);
  assert.match(html, /role="alert"/);
  assert.match(html, /aria-label="Provider error 401"/);
  assert.match(html, /data-status="401"/);
  assert.match(html, /provider returned 401: a message from the provider/);
  assert.match(html, /Request id: <code>req-401<\/code>/);
  assert.match(html, /Copy diagnostics/);
  assert.match(html, /class="provider-error"/);
});

test("a 429 remedy names the wait and a 5xx remedy names a wait before retry", () => {
  const limited = parseProviderError(body(429, { retry_after: 12 }));
  assert.ok(limited);
  assert.match(limited.remedy, /Wait 12 s/);
  assert.match(limited.remedy, /rate-limited/);

  const failed = parseProviderError(body(500));
  assert.ok(failed);
  assert.match(failed.remedy, /Wait a moment/);
  assert.match(failed.remedy, /server error/);
  assert.match(failed.remedy, /did not switch doors/);
});

test("copy diagnostics is exact text and never carries a credential", () => {
  const error = parseProviderError(body(503, { api_key: "sk-secret-do-not-copy" }));
  assert.ok(error);
  const text = providerErrorDiagnostics(error, "turn-7");
  assert.equal(
    text,
    [
      "provider error 503",
      "provider OpenRouter / anthropic/claude-sonnet-4.5",
      "request req-503",
      "message provider returned 503: a message from the provider",
      `remedy ${error.remedy}`,
      "turn turn-7",
    ].join("\n"),
  );
  assert.doesNotMatch(text, /sk-secret-do-not-copy/);
  assert.equal(providerErrorDiagnostics(error).includes("turn "), false);
});

test("an ordinary error keeps the plain error line instead of a provider panel", () => {
  const html = renderToStaticMarkup(<ProviderErrorPanel body={{ message: "no steps left" }} />);
  assert.equal(html, "");
  assert.equal(parseProviderError({ message: "no steps left" }), null);
});
