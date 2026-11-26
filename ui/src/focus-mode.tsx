/**
 * Roadmap 120: focus mode.
 *
 * The shell drops its rail, list, inspector and utility chrome while the detail pane — and
 * therefore the composer and any approval card — stays put. The exit button lives in the status
 * line, which focus mode never hides, and carries the same binding the palette advertises.
 * The class decision and the chip are here so both the renderer tests and the window use one
 * definition of what focus does.
 */

/** Direct children of `.shell` that focus mode removes. The detail pane is deliberately absent. */
export const FOCUS_HIDDEN = ["rail", "list-pane", "inspector", "utility-cluster", "pane-handle"];

/** The view classes the shell needs before focus is considered. Order matters for readability. */
export function shellClass({
  native,
  view,
  focus,
}: {
  native: boolean;
  view: string;
  focus: boolean;
}): string {
  const classes = ["shell"];
  if (native) classes.push("chrome");
  if (view === "session") classes.push("session");
  if (view === "settings" || view === "account") classes.push("settings");
  if (view === "diff") classes.push("diff");
  if (view === "home") classes.push("home");
  if (view === "shelf") classes.push("shelf");
  if (view === "chat") classes.push("chat");
  if (focus) classes.push("focus");
  return classes.join(" ");
}

export function focusLabel(focus: boolean): string {
  return focus ? "Exit focus" : "Focus";
}

export default function FocusChip({
  focus,
  shortcut,
  onToggle,
}: {
  focus: boolean;
  shortcut: string;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      className="chip focus-chip"
      onClick={onToggle}
      aria-pressed={focus}
      title={`Hide the rail, list and inspector (${shortcut})`}
    >
      {focusLabel(focus)} · {shortcut}
    </button>
  );
}
