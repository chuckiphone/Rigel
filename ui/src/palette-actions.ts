/** The palette's fixed navigation actions, shared by the data module and the component. */

export type PaletteAction =
  | "home"
  | "shelf"
  | "attention"
  | "session"
  | "chat"
  | "diff"
  | "account"
  | "settings"
  | "focus";

export interface Action {
  id: PaletteAction;
  label: string;
  detail: string;
}

export const ACTIONS: Action[] = [
  { id: "home", label: "Home", detail: "The Rigel Paper — what the agents did today" },
  { id: "focus", label: "Focus mode", detail: "Hide the rail, list and inspector" },
  { id: "shelf", label: "Shelf", detail: "Search the local skill index" },
  { id: "attention", label: "Attention", detail: "What still needs an answer" },
  { id: "session", label: "Session", detail: "Transcript, agents and composer" },
  { id: "chat", label: "Chat", detail: "The agent room" },
  { id: "diff", label: "Diff", detail: "Claims, patches and merges" },
  { id: "account", label: "Account", detail: "Identity, verification and session sync" },
  { id: "settings", label: "Settings", detail: "Providers, safety, appearance" },
];
