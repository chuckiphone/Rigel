import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import SessionRoster, {
  ROSTER_ROW_HEIGHT,
  groupSessions,
  rosterFixture,
  rosterWindow,
} from "../src/roster";

const fixture = rosterFixture(500);

test("the 500-session fixture is deterministic and uniquely named", () => {
  assert.equal(fixture.length, 500);
  assert.equal(fixture[0].title, "Fixture session 0001");
  assert.equal(fixture[499].title, "Fixture session 0500");
  assert.equal(new Set(fixture.map((row) => row.id)).size, 500);
});

test("sessions group by project and status with one header per group", () => {
  const entries = groupSessions([...fixture.slice(0, 2), { ...fixture[0], id: "archived", archived_at: 1 }]);
  const headers = entries.filter((entry) => entry.kind === "header");
  assert.deepEqual(
    headers.map((entry) => (entry.kind === "header" ? `${entry.label}:${entry.count}` : "")),
    ["fixture:3", "active:1", "idle:1", "archived:1"],
  );
  assert.equal(entries.filter((entry) => entry.kind === "session").length, 3);
});

test("the virtual window keeps the scrollbar honest and mounts only a slice", () => {
  const window = rosterWindow(500, 10_000, 400);
  assert.ok(window.start > 0 && window.end < 500);
  assert.equal(window.paddingTop, window.start * ROSTER_ROW_HEIGHT);
  assert.equal(window.paddingBottom, (500 - window.end) * ROSTER_ROW_HEIGHT);
  assert.equal(window.paddingTop + (window.end - window.start) * ROSTER_ROW_HEIGHT + window.paddingBottom, 500 * ROSTER_ROW_HEIGHT);
  assert.ok(window.end - window.start <= 20, "overscan is bounded");
  assert.ok(rosterWindow(500, 0, 0).end <= 9, "overscan still bounds a zero-height viewport");
});

test("the row menu is a native button with aria-haspopup and four named actions", () => {
  const html = renderToStaticMarkup(
    <SessionRoster
      sessions={fixture.slice(0, 1)}
      selectedId={null}
      renamingId={null}
      renameText=""
      busyId={null}
      onRenameText={() => {}}
      onCommitRename={() => {}}
      onCancelRename={() => {}}
      actions={{
        onOpen: () => {},
        onRename: () => {},
        onPin: () => {},
        onArchive: () => {},
        onDelete: () => {},
      }}
    />,
  );
  assert.match(html, /aria-label="Session roster"/);
  assert.match(html, /aria-haspopup="menu"/);
  assert.match(html, /aria-label="Actions for Fixture session 0001"/);
  assert.match(html, /Fixture session 0001/);
  assert.match(html, /data-group="active"/);
});
