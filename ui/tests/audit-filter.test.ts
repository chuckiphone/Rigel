import assert from "node:assert/strict";
import test from "node:test";

import {
  EMPTY_AUDIT_FILTER,
  auditFilterFromHash,
  auditFilterIsEmpty,
  auditHash,
} from "../src/audit-filter";

test("the audit filter round-trips through the settings hash", () => {
  const filter = {
    project: "rigel",
    q: "echo audited",
    session: "session-1",
    agent: "agent-2",
    turn: "turn-3",
    decision: "denied",
  };
  const hash = auditHash(filter);
  assert.equal(
    hash,
    "#settings/audit?project=rigel&q=echo+audited&session=session-1&agent=agent-2&turn=turn-3&decision=denied",
  );
  assert.deepEqual(auditFilterFromHash(hash), filter);
});

test("empty fields stay out of the address and are restored as empty", () => {
  const hash = auditHash({ ...EMPTY_AUDIT_FILTER, session: "s1" });
  assert.equal(hash, "#settings/audit?session=s1");
  assert.deepEqual(auditFilterFromHash(hash), { ...EMPTY_AUDIT_FILTER, session: "s1" });
  assert.equal(auditFilterFromHash("#settings/audit").session, "");
  assert.ok(auditFilterIsEmpty(EMPTY_AUDIT_FILTER));
  assert.ok(!auditFilterIsEmpty({ ...EMPTY_AUDIT_FILTER, decision: "allowed" }));
});