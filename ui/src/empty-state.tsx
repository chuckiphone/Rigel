/**
 * Roadmap 117: contextual empty states.
 *
 * An empty surface is a control, not a picture. Each surface names the one action that can make
 * something happen next, derived from whether rows are hidden by a filter or nothing exists yet.
 * There is no illustration where a button belongs, and a loading state gets no action at all.
 */

export type EmptySurface =
  | "sessions"
  | "claims"
  | "conflicts"
  | "queue"
  | "plugins"
  | "memory"
  | "chat"
  | "ledger"
  | "patches"
  | "worktree"
  | "concord";

export interface EmptySignals {
  /** Rows exist but a filter hides them: the honest action is to clear it. */
  filtered?: boolean;
  /** A read is still in flight: say so, and offer nothing that could act on stale rows. */
  loading?: boolean;
}

export type EmptyActionId =
  | "start-session"
  | "clear-filter"
  | "open-diff"
  | "open-shelf"
  | "open-settings"
  | "open-chat";

export interface EmptyAction {
  id: EmptyActionId;
  label: string;
}

/** Every empty surface has exactly one action; the state decides which one. */
export function emptyAction(surface: EmptySurface, signals: EmptySignals = {}): EmptyAction | null {
  if (signals.loading) return null;
  if (signals.filtered) return { id: "clear-filter", label: "Clear filters" };
  switch (surface) {
    case "sessions":
      return { id: "start-session", label: "Start a session" };
    case "claims":
    case "conflicts":
    case "queue":
      return { id: "open-diff", label: "Open Diff" };
    case "plugins":
      return { id: "open-shelf", label: "Open the skill shelf" };
    case "memory":
    case "ledger":
      return { id: "open-settings", label: "Open Settings" };
    case "chat":
      return { id: "open-chat", label: "Open Chat" };
    case "patches":
    case "worktree":
    case "concord":
      return { id: "start-session", label: "Start a session" };
  }
}

export const EMPTY_COPY: Record<EmptySurface, string> = {
  sessions: "No session in this project yet. Start one with the first prompt.",
  claims: "No agent has claimed a file yet.",
  conflicts: "No conflicts. That is the normal state, not a guarantee.",
  queue: "No writer is queued. The next write takes its path's lane.",
  plugins: "No plugin matches this filter.",
  memory: "No committed memory facts yet.",
  chat: "No bot is configured yet.",
  ledger: "No approvals have been recorded in this project yet.",
  patches: "No patch has been applied in this session.",
  worktree: "No subagent has been given write access in this session.",
  concord: "Nothing is waiting for a reply.",
};

export default function EmptyState({
  surface,
  signals = {},
  copy,
  onAction,
}: {
  surface: EmptySurface;
  signals?: EmptySignals;
  /** A caller can keep its existing sentence; the action still comes from the surface state. */
  copy?: string;
  onAction: (action: EmptyActionId) => void;
}) {
  const action = emptyAction(surface, signals);
  return (
    <div className="empty-state" data-surface={surface}>
      <p className="fine">{copy ?? EMPTY_COPY[surface]}</p>
      {action ? (
        <button type="button" className="chip empty-action" onClick={() => onAction(action.id)}>
          {action.label}
        </button>
      ) : null}
    </div>
  );
}
