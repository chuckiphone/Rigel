# Slash commands

A draft that starts with `/` is a command, not a prompt. It is parsed locally before any turn,
review card or model call. An unknown command never reaches the model: the composer shows a named
error and keeps the turn count unchanged.

## Grammar

```
/<lowercase-name>[ <arguments>]
```

- The command must be the first non-whitespace character of the draft.
- A name is one or more lowercase letters, digits or hyphens: `/model`, `/clear`, `/set-model`.
- Arguments are optional, separated from the name by whitespace, and may contain spaces.
- Only the first line is parsed; a newline ends the command.
- A draft that does not start with `/` is an ordinary prompt, including text like `use /help here`.

## Named error

An unknown name or a malformed first line produces:

```
Unknown command /<name>. Run /help for the grammar: /<lowercase-name>[ <arguments>]
```

The error is shown in the composer's error line and in the notice area. The draft is not sent and
no turn row is created.

## Commands

| Command | Effect |
|---|---|
| `/help` | Show this table in the notice area. |
| `/clear` | Clear the draft without sending a turn. |
| `/focus` | Toggle focus mode. |
| `/plan` | Switch the next turn to plan mode. |
| `/act` | Switch the next turn to act mode. |
| `/model` | Open the model picker. |
| `/diff` | Open the diff review. |
| `/session` | Open the session surface. |
| `/skill <name>` | Pin a named skill into this session. |

A command that only changes the window (`/clear`, `/focus`, `/model`) never calls a model. Commands
that act on a session do so through the same handlers the buttons use, so the palette and the
composer cannot take different paths.
