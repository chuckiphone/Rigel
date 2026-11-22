---
name: ag-ui
description: Map a Rigel agent run onto AG-UI events so a client can render tokens, tool calls and approvals live instead of one finished answer.
license: MIT
metadata:
  author: Rigel
  tags:
    - ag-ui
---

# AG-UI event mapping

## Purpose


AG-UI is an event protocol, not a runtime to adopt wholesale. Rigel already emits typed
deltas; this skill gives them the standard names so one client can render any run.

## Event vocabulary

Emit a run envelope and typed events inside it:

| AG-UI event | Rigel source | Notes |
|---|---|---|
| `RUN_STARTED` | turn start | carries `runId`, `threadId` |
| `TEXT_MESSAGE_START` | first `Delta::Text` of a message | one per assistant message |
| `TEXT_MESSAGE_CONTENT` | each `Delta::Text` | the delta, not the accumulated text |
| `TEXT_MESSAGE_END` | message boundary | before a tool call or the final answer |
| `TOOL_CALL_START` | `Delta::ToolCall` | carries the tool name and call id |
| `TOOL_CALL_ARGS` | streamed arguments | may arrive in fragments |
| `TOOL_CALL_END` | tool result row | pair by call id |
| `STATE_SNAPSHOT` | session roster, bill, step count | send on connect and after a change |
| `RUN_FINISHED` | turn stop reason | include the stop reason |
| `RUN_ERROR` | failed turn | include the error code, never a credential |

## Rules that keep the stream honest

- One `runId` per turn and one `threadId` per session; never reuse a run id after a
  reconnect.
- `TOOL_CALL_END` is emitted even when the tool failed. A client must not infer success
  from the absence of an error event.
- An approval is a state change, not a message: send a fresh `STATE_SNAPSHOT` when a card
  opens or is answered.
- A resumed turn is a new run with the same thread. Do not replay the old run's text as if
  it were live.
- Redact before serialising. The event stream is a display surface, not a debug log.

## Integration check

Drive one scripted turn and assert this order:

```text
RUN_STARTED → TEXT_MESSAGE_START → TEXT_MESSAGE_CONTENT* → TEXT_MESSAGE_END
→ TOOL_CALL_START → TOOL_CALL_ARGS* → TOOL_CALL_END → TEXT_MESSAGE_START … → RUN_FINISHED
```

A tool call without a following end event is a bug in the adapter, not a client concern.

## Boundaries

- Do not add an AG-UI server dependency to the core for this mapping; the protocol is the
  useful part.
- Do not put raw provider payloads in an event. They contain prompts and sometimes keys.
