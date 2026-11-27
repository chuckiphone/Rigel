/**
 * Roadmap 104: compare two recorded turns side by side.
 *
 * Both columns are built from the same transcript data the session already holds: the turn row
 * (prompt, route/model), its usage row and its tool-call items. Rows line up by name, so a
 * differing field is directly opposite its counterpart and can be highlighted without guessing
 * from character offsets. Scrolling is anchored to the row under the pointer: both columns move
 * together, and the anchor row keeps its place in view.
 */

import { useEffect, useRef, useState, type ReactNode } from "react";

import type { Item, TurnTranscript, Usage } from "./api";
import { fromUsage } from "./inspector/request";

/** One comparable fact of a turn. `value` is the displayed text; `key` names the row. */
export interface CompareField {
  key: string;
  label: string;
  value: string;
}

function text(item: Item, key: string): string {
  const value = item.body?.[key];
  return typeof value === "string" ? value : "";
}

function record(item: Item, key: string): Record<string, unknown> | null {
  const value = item.body?.[key];
  return value && typeof value === "object" ? (value as Record<string, unknown>) : null;
}

function toolCall(item: Item): string {
  const args = record(item, "arguments");
  if (!args) return text(item, "text").replace(/\s+/g, " ").trim();
  const name = text(item, "tool") || text(item, "name") || item.kind;
  const arg = Object.entries(args)
    .map(([key, value]) => `${key}=${typeof value === "string" ? value : JSON.stringify(value)}`)
    .join(" ");
  return `${name}(${arg})`;
}

function outputs(turn: TurnTranscript): string[] {
  return turn.items
    .filter((item) => item.kind === "message" || item.kind === "note")
    .map((item) => text(item, "text").replace(/\s+/g, " ").trim())
    .filter((value) => value.length > 0);
}

/** Route and model come from the recorded request inspector, so the two views cannot disagree. */
export function compareFields(turn: TurnTranscript): CompareField[] {
  const usage: Usage | null = turn.turn.usage;
  const route = fromUsage(usage ? { ...usage } : {}).route;
  const tools = turn.items.filter((item) =>
    ["command", "mcp_call", "web_search", "file_change"].includes(item.kind),
  );
  return [
    { key: "prompt", label: "Prompt", value: turn.turn.prompt?.trim() || "—" },
    {
      key: "route",
      label: "Route",
      value: [route.provider, route.model].filter(Boolean).join(" / ") || "—",
    },
    {
      key: "model",
      label: "Served model",
      value: usage?.served_model || "—",
    },
    { key: "tools", label: "Tool calls", value: tools.length ? tools.map(toolCall).join("\n") : "—" },
    { key: "output", label: "Output", value: outputs(turn).join("\n") || "—" },
    {
      key: "cost",
      label: "Cost",
      value: usage ? `$${(usage.cost_micros / 1_000_000).toFixed(6)}` : "—",
    },
  ];
}

/** The rows whose values differ, in a stable order. A shared testable helper, not view state. */
export function differingKeys(left: CompareField[], right: CompareField[]): string[] {
  const byKey = new Map(right.map((field) => [field.key, field.value]));
  return left
    .filter((field) => byKey.get(field.key) !== field.value)
    .map((field) => field.key);
}

function Column({
  title,
  fields,
  differing,
  register,
  onAnchor,
}: {
  title: string;
  fields: CompareField[];
  differing: Set<string>;
  register: (element: HTMLDivElement | null) => void;
  onAnchor: (index: number) => void;
}): ReactNode {
  return (
    <div
      className="compare-column"
      ref={register}
      onScroll={(event) => {
        const rows = Array.from(event.currentTarget.querySelectorAll<HTMLElement>(".compare-row"));
        const anchor = rows.findIndex((row) => row.offsetTop >= event.currentTarget.scrollTop);
        onAnchor(Math.max(0, anchor));
      }}
    >
      <h4 className="compare-title">{title}</h4>
      <dl className="compare-rows">
        {fields.map((field, index) => (
          <div
            className={`compare-row${differing.has(field.key) ? " differing" : ""}`}
            data-field={field.key}
            key={`${field.key}-${index}`}
          >
            <dt>{field.label}</dt>
            <dd>{field.value}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

export default function CompareTurns({
  left,
  right,
  onClose,
}: {
  left: TurnTranscript;
  right: TurnTranscript;
  onClose?: () => void;
}): ReactNode {
  const leftFields = compareFields(left);
  const rightFields = compareFields(right);
  const differing = new Set(differingKeys(leftFields, rightFields));
  const columns = useRef<(HTMLDivElement | null)[]>([]);
  const syncing = useRef(false);
  const [anchor, setAnchor] = useState<{ index: number; offset: number }>({ index: 0, offset: 0 });

  // Keep both columns locked to the same row: one scroll sets the anchor, the other follows it.
  useEffect(() => {
    if (syncing.current) return;
    syncing.current = true;
    for (const column of columns.current) {
      const row = column?.querySelectorAll<HTMLElement>(".compare-row")[anchor.index];
      if (column && row) column.scrollTop = row.offsetTop - anchor.offset;
    }
    syncing.current = false;
  }, [anchor]);

  const onAnchor = (index: number) => {
    const column = columns.current[0];
    const row = column?.querySelectorAll<HTMLElement>(".compare-row")[index];
    setAnchor({ index, offset: row ? row.offsetTop - (column?.scrollTop ?? 0) : 0 });
  };

  return (
    <section className="compare-turns" aria-label="Compare turns">
      <header className="compare-head">
        <strong>
          Turn {left.turn.seq} vs turn {right.turn.seq}
        </strong>
        <span className="fine">
          {differing.size} {differing.size === 1 ? "field" : "fields"} differ
        </span>
        {onClose ? (
          <button type="button" onClick={onClose} aria-label="Close turn comparison">
            Close
          </button>
        ) : null}
      </header>
      <div className="compare-grid">
        <Column
          title={`Turn ${left.turn.seq}`}
          fields={leftFields}
          differing={differing}
          register={(element) => {
            columns.current[0] = element;
          }}
          onAnchor={onAnchor}
        />
        <Column
          title={`Turn ${right.turn.seq}`}
          fields={rightFields}
          differing={differing}
          register={(element) => {
            columns.current[1] = element;
          }}
          onAnchor={() => {}}
        />
      </div>
    </section>
  );
}
