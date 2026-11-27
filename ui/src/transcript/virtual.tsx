/**
 * Roadmap 186: transcript virtualisation.
 *
 * A long transcript renders only the rows in the viewport plus a small overscan; the two spacers
 * carry the height of everything else so the scrollbar stays honest. Resizing keeps the centred
 * row centred, and a jump lands on the named row instead of an approximate offset. The helpers are
 * pure so both invariants are testable without a browser.
 */

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

export interface VirtualWindow {
  start: number;
  end: number;
  paddingTop: number;
  paddingBottom: number;
}

/** The half-open row range to render, with spacers for the rows outside it. */
export function virtualWindow(
  count: number,
  scrollTop: number,
  viewport: number,
  rowHeight: number,
  overscan = 4,
): VirtualWindow {
  if (count <= 0 || rowHeight <= 0) {
    return { start: 0, end: 0, paddingTop: 0, paddingBottom: 0 };
  }
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const span = Math.ceil(Math.max(0, viewport) / rowHeight) + overscan * 2 + 1;
  const start = Math.min(first, count - 1);
  const end = Math.min(count, start + span);
  return { start, end, paddingTop: start * rowHeight, paddingBottom: (count - end) * rowHeight };
}

/** The row at the centre of the viewport; this is what a resize must keep in place. */
export function centeredIndex(scrollTop: number, rowHeight: number, viewport: number): number {
  if (rowHeight <= 0) return 0;
  return Math.max(0, Math.floor((scrollTop + viewport / 2) / rowHeight));
}

/** Scroll offset that puts a row at the centre, used by jumps and by resize anchoring. */
export function centeredScroll(index: number, rowHeight: number, viewport: number): number {
  return Math.max(0, Math.round(index * rowHeight + rowHeight / 2 - viewport / 2));
}

export interface VirtualTranscriptProps<T> {
  rows: readonly T[];
  rowHeight?: number;
  overscan?: number;
  getKey: (row: T, index: number) => string;
  renderRow: (row: T, index: number) => ReactNode;
  /** Change this to land on a row; the row is centred rather than placed at the top edge. */
  jumpTo?: number | null;
  label?: string;
  className?: string;
}

export default function VirtualTranscript<T>({
  rows,
  rowHeight = 40,
  overscan = 4,
  getKey,
  renderRow,
  jumpTo,
  label = "Virtual transcript",
  className,
}: VirtualTranscriptProps<T>) {
  const scroller = useRef<HTMLDivElement>(null);
  const anchor = useRef<number | null>(null);
  const [viewport, setViewport] = useState(600);
  const [scrollTop, setScrollTop] = useState(0);

  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    setViewport(element.clientHeight);
    const observer = new ResizeObserver(() => setViewport(element.clientHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const element = scroller.current;
    if (!element || anchor.current === null) return;
    const target = centeredScroll(anchor.current, rowHeight, viewport);
    if (Math.abs(element.scrollTop - target) > 1) element.scrollTop = target;
    setScrollTop(element.scrollTop);
  }, [viewport, rowHeight]);

  useEffect(() => {
    if (jumpTo === null || jumpTo === undefined) return;
    const element = scroller.current;
    if (!element) return;
    element.scrollTop = centeredScroll(jumpTo, rowHeight, viewport);
    anchor.current = jumpTo;
    setScrollTop(element.scrollTop);
  }, [jumpTo, rowHeight, viewport]);

  const window = useMemo(
    () => virtualWindow(rows.length, scrollTop, viewport, rowHeight, overscan),
    [rows.length, scrollTop, viewport, rowHeight, overscan],
  );

  return (
    <div
      className={`virtual-transcript${className ? ` ${className}` : ""}`}
      ref={scroller}
      role="region"
      aria-label={label}
      onScroll={(event) => {
        const top = event.currentTarget.scrollTop;
        setScrollTop(top);
        anchor.current = centeredIndex(top, rowHeight, viewport);
      }}
    >
      <div className="virtual-spacer" style={{ height: window.paddingTop }} aria-hidden="true" />
      {rows.slice(window.start, window.end).map((row, offset) => {
        const index = window.start + offset;
        return (
          <div
            className="virtual-row"
            key={getKey(row, index)}
            data-index={index}
            style={{ height: rowHeight }}
          >
            {renderRow(row, index)}
          </div>
        );
      })}
      <div className="virtual-spacer" style={{ height: window.paddingBottom }} aria-hidden="true" />
    </div>
  );
}

/** The browser preview's 10,000-row fixture, with a jump control for the window invariant. */
export function VirtualTranscriptPreview({ enabled }: { enabled: boolean }) {
  const [jump, setJump] = useState<number | null>(null);
  const rows = useMemo(
    () =>
      Array.from({ length: 10_000 }, (_, index) => ({
        id: `virtual-${index}`,
        seq: index + 1,
        text: index % 7 === 0 ? "prompt" : index % 5 === 0 ? "tool result" : "answer",
      })),
    [],
  );
  if (!enabled) return null;
  return (
    <section className="virtual-preview" aria-label="Transcript virtualisation preview">
      <div className="virtual-preview-head">
        <span className="fine">{rows.length.toLocaleString()} turns · only the visible window is in the DOM</span>
        <button type="button" className="chip" onClick={() => setJump(8_999)}>
          Jump to turn 9000
        </button>
      </div>
      <VirtualTranscript
        rows={rows}
        rowHeight={40}
        getKey={(row) => row.id}
        renderRow={(row) => (
          <span className="virtual-row-body">
            <span className="mono">turn {row.seq}</span>
            <span className="faint">{row.text}</span>
          </span>
        )}
        jumpTo={jump}
      />
    </section>
  );
}
