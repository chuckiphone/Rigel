/**
 * Memory inspector (roadmap 55).
 *
 * The core is the only ranker. This panel renders the rows it returns, including the three
 * factors and their reasons, and offers the two actions that change a rank: a recall (which
 * counts the returned facts as used) and a pin (which stops the recency value from decaying).
 * It never computes a score of its own.
 */

import { useCallback, useEffect, useState } from "react";
import { api, type MemoryFact } from "../api";
import AsyncState from "../async-state";

export function MemoryInspector({ project }: { project: string }) {
  const [facts, setFacts] = useState<MemoryFact[]>([]);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const report = useCallback((failure: unknown) => {
    setError(failure instanceof Error ? failure.message : "Memory ranking failed.");
  }, []);

  const load = useCallback(() => {
    setBusy(true);
    api
      .memoryFacts(project)
      .then((rows) => {
        setFacts(rows);
        setError("");
      })
      .catch(report)
      .finally(() => setBusy(false));
  }, [project, report]);

  useEffect(() => {
    load();
  }, [load]);

  const recall = () => {
    setBusy(true);
    api
      .recallMemory(project, query, undefined, 8)
      .then((rows) => {
        setFacts(rows);
        setError("");
      })
      .catch(report)
      .finally(() => setBusy(false));
  };

  const pin = (fact: MemoryFact) => {
    api
      .pinMemoryFact(project, fact.id, !fact.pinned)
      .then(load)
      .catch(report);
  };

  return (
    <section aria-label="Memory facts">
      <h3>Memory</h3>
      <p className="fine">
        Every non-heading line in committed memory is a fact. The score is recency 0.50 + use
        0.30 + pin 0.20; a pinned fact never decays.
      </p>
      <div className="cap-form">
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") recall();
          }}
          placeholder="recall…"
          aria-label="Recall memory facts"
        />
        <button className="chip" onClick={recall} disabled={busy || !project}>
          Recall
        </button>
      </div>
      {error ? <AsyncState state="error" label={error} /> : null}
      {facts.length === 0 ? (
        <AsyncState
          state={busy ? "loading" : "empty"}
          label={busy ? "Ranking committed memory…" : "No committed memory facts yet."}
        />
      ) : (
        <ul className="memory-fact-list">
          {facts.slice(0, 8).map((fact) => (
            <li key={fact.id}>
              <div className="memory-fact-head">
                <span className="mono">{fact.score.total.toFixed(3)}</span>
                <span className="tag">
                  {fact.agent_id}/{fact.topic}
                </span>
                <button className="link" onClick={() => void pin(fact)}>
                  {fact.pinned ? "unpin" : "pin"}
                </button>
              </div>
              <p className="memory-fact-text">{fact.text}</p>
              <dl className="memory-factors">
                {fact.score.factors.map((factor) => (
                  <div key={factor.name}>
                    <dt>{factor.name}</dt>
                    <dd title={factor.reason}>
                      {factor.value.toFixed(2)} × {factor.weight.toFixed(2)}
                    </dd>
                  </div>
                ))}
              </dl>
              <p className="fine memory-reasons">
                {fact.score.factors
                  .map((factor) => `${factor.name}: ${factor.reason}`)
                  .join(" · ")}
              </p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
