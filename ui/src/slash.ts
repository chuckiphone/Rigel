/**
 * Roadmap 113: the slash-command grammar.
 *
 * A command occupies the whole first line: `/name [arguments]`. Names are lowercase letters,
 * digits and hyphens. A name that is not in the table is a named error, never a prompt: sending a
 * typo to a model would spend money on a question the user did not ask.
 */

export interface SlashCommand {
  name: string;
  args: string;
  detail: string;
}

export type SlashParse =
  | { kind: "prompt"; text: string }
  | { kind: "command"; command: SlashCommand }
  | { kind: "unknown"; name: string; error: string };

export const SLASH_COMMANDS: Record<string, string> = {
  help: "List the commands Rigel understands.",
  clear: "Clear the current draft without sending a turn.",
  focus: "Toggle focus mode.",
  plan: "Switch the next turn to plan mode.",
  act: "Switch the next turn to act mode.",
  model: "Open the model picker.",
  diff: "Open the diff review.",
  session: "Open the session surface.",
  skill: "Pin a named skill into this session.",
};

export const GRAMMAR = "/<lowercase-name>[ <arguments>]";

export function unknownCommandError(name: string): string {
  return `Unknown command /${name}. Run /help for the grammar: ${GRAMMAR}`;
}

/** Parse one draft. Only a leading slash makes it a command; an ordinary prompt passes through. */
export function parseSlash(draft: string): SlashParse {
  const text = draft.trim();
  if (!text.startsWith("/")) return { kind: "prompt", text };
  const firstLine = text.split(/\r?\n/, 1)[0].trim();
  const match = /^\/([a-z0-9-]+)(?:\s+(.*))?$/.exec(firstLine);
  if (!match) {
    const name = firstLine.slice(1).split(/\s/, 1)[0] || firstLine.slice(1);
    return { kind: "unknown", name, error: unknownCommandError(name) };
  }
  const [, name, args = ""] = match;
  if (!(name in SLASH_COMMANDS)) {
    return { kind: "unknown", name, error: unknownCommandError(name) };
  }
  return {
    kind: "command",
    command: { name, args: args.trim(), detail: SLASH_COMMANDS[name] },
  };
}

/** The exact text shown by /help, in table order. */
export function helpText(): string {
  return Object.entries(SLASH_COMMANDS)
    .map(([name, detail]) => `/${name} — ${detail}`)
    .join("\n");
}
