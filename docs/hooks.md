# Optional git hooks

`rigel hooks install --project <dir>` writes three hooks into a project's `.git/hooks`.
The suite is opt-in: nothing is installed until you run that command. Every body has an
`--json` form and opens no socket.

| Hook | Body | What it does |
|---|---|---|
| `pre-commit` | `rigel hooks scan --project <dir>` | Runs the same secret scanner Rigel uses before a write over `git diff --cached`. A finding on an added line blocks the commit. |
| `prepare-commit-msg` | `rigel hooks trailer --file "$1"` | Adds exactly one `rigel: <agent>` trailer. The agent comes from `--agent`, then `RIGEL_AGENT_ID`, then `cli`. |
| `commit-msg` | `rigel hooks check-msg --file "$1"` | Attribution check: refuses a message with no well-formed `rigel: <agent>` trailer. |

The trailer token is case-insensitive. Its value is reduced to one safe line (no whitespace,
newline or colon), so a label cannot inject another trailer. An existing message, including its
own trailers, is otherwise preserved byte for byte. A finding reports the rule, line, column and
width, never the matched secret text.

## Install and uninstall

Installation backs up every hook that already existed into `.rigel/hooks/<name>.orig` and records
its content hash and Unix mode in `.rigel/hooks.json`. A linked worktree (whose `.git` is a
`gitdir:` pointer) resolves to its real git directory.

`rigel hooks uninstall --project <dir>` restores each previous hook byte for byte, including its
mode, or removes the hooks Rigel added. It first checks that every installed hook still matches
the recorded hash. If you edited one after install, uninstall refuses and leaves everything in
place so it cannot overwrite your edit; restore the file by hand and remove `.rigel/hooks.json`
when you are ready.

`rigel hooks status --project <dir>` shows which hooks are installed, which have a backup, and
which no longer match what install wrote.
