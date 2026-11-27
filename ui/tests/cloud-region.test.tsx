import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { DataLocationPanel, regionMoveConfirmation, regionMoveConfirmed } from "../src/cloud-region";

test("data-location panel reports the stored region for the configured project", () => {
  const html = renderToStaticMarkup(
    <DataLocationPanel
      status={{ configured: true, project_id: "demo-project", data_region: "europe-west1" }}
      onPin={() => {}}
    />,
  );
  assert.match(html, /aria-label="Data location"/);
  assert.match(html, /europe-west1/);
  assert.match(html, /demo-project/);
});

test("cross-region confirmation is project and destination specific", () => {
  assert.equal(
    regionMoveConfirmation("demo-project", "us-central1", "europe-west1"),
    "MOVE demo-project FROM us-central1 TO europe-west1",
  );
  assert.equal(regionMoveConfirmed("demo-project", "us-central1", "europe-west1", "yes"), false);
  assert.equal(
    regionMoveConfirmed(
      "demo-project",
      "us-central1",
      "europe-west1",
      "MOVE demo-project FROM us-central1 TO europe-west1",
    ),
    true,
  );
  assert.equal(regionMoveConfirmed("demo-project", "us-central1", "us-central1", ""), true);
});
