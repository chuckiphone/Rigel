# Local metrics

`rigel stats --project <dir> [--json]` reports one aggregate over the turns recorded in a
project's `.rigel/rigel.db`. It reads the same `SessionService` the window uses, opens no socket
and makes no request. There is no upload action and no telemetry: the numbers stay on this machine
until you copy the output yourself.

`--json` prints one object with a stable `schema` of `1`. A human-readable run prints the same
fields as labelled lines. Empty projects return zeroes, not an error.

```json
{
  "schema": 1,
  "sessions": 1,
  "turns": 2,
  "completed_turns": 1,
  "steps": 3,
  "latency_ms": { "total_ms": 450, "mean_ms": 450, "max_ms": 450 },
  "model_ms": 420,
  "queue_ms": 70,
  "tokens": { "input": 110, "output": 45, "reasoning": 10, "total": 165 },
  "cache": { "hits": 1, "read_tokens": 20, "write_tokens": 5 },
  "retries": 1,
  "cost_micros": 230
}
```

| Field | Meaning |
|---|---|
| `schema` | Shape version; consumers should refuse an unknown value. |
| `sessions`, `turns`, `completed_turns` | Rows read from the project, with only `completed` turns counted in the last two latency figures. |
| `steps` | Number of recorded usage rows; one per model call, including partial ones. |
| `latency_ms` | Wall time of completed turns: `total_ms` is the sum, `mean_ms` the average over completed turns, `max_ms` the longest. A turn without `completed_at` contributes no latency. |
| `model_ms`, `queue_ms` | `duration_ms` and `queue_ms` summed from usage rows: time inside the model call and time between step boundaries. |
| `tokens` | Recorded input, output and reasoning tokens; `total` is their sum. Cached reads are a subset of input, never extra tokens. |
| `cache` | `hits` counts usage rows where the provider reported a read; `read_tokens` and `write_tokens` are the reported token counts. A provider that reported nothing is not a hit. |
| `retries` | Count of recorded `retry` items, not failed turns. |
| `cost_micros` | Sum of the authoritative micro-dollar costs on the usage rows. |

Every value comes from local rows added by a finished or interrupted turn. The command never
derives a figure from a provider response at report time, so a network outage cannot change the
numbers.
