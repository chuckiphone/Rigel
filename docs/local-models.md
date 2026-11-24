# Local models

Rigel has three keyless doors on the loopback interface: Ollama (`local`), llamafile
(`llamafile`) and llama.cpp's `llama-server` (`llamacpp`). All three speak the OpenAI-compatible
contract, so a turn reaches them through the same provider table as every hosted door — the only
difference is that there is no credential, and no outbound request leaves the machine.

This page covers the `llamacpp` door, which can also start and stop the server it talks to. The
reference profile is a 35B-A3B mixture-of-experts GGUF on a 6 GB laptop GPU:

| Piece | Value |
|---|---|
| Model | a Qwen3.6-35B-A3B abliterated GGUF (`Q4_K`, ~20.2 GiB, 40 layers, 256 experts) |
| Runtime | llama.cpp b12640, CUDA 12.4 Windows build (`llama-server.exe`) |
| Launch | `-ngl 99 --n-cpu-moe 36 -c 4096 -fa on -t 8 -lm mmap+mlock --host 127.0.0.1 --port 8080` |
| Expected | 21–26 tok/s pinned, 11–17 tok/s unpinned; prompt processing ~20–30 tok/s |

Rigel never downloads, converts or re-quantises the weights. Those flags were measured on the
reference machine, and changing `--n-cpu-moe`, `-t`, `-fa` or `-lm` without re-measuring trades a
working turn for an out-of-memory window or a halved token rate.

## Configure, start, stop

```bash
rigel local setup --server "C:\llama\llama-server.exe" --model "C:\models\qwen.gguf"
rigel local start      # waits for /health, then reports the mode it chose
rigel local status     # state, base URL, mode, PID and the one sentence that explains it
rigel local stop       # stops the recorded PID, after checking the image is llama-server
```

`rigel local setup` with no flags looks for `llama-server` on `PATH` and then in the local model
harness (`~/.config/freebuff-desktop/local-projects/*/tools/llamacpp`), and for the largest GGUF
under `.../models/gguf`. The profile is saved as `llama-server.json` beside the index.

In the window, the same three actions are on **Settings → Providers → llama.cpp**. Choosing that
door and starting the server means the picker reads its `/v1/models` roster, the turn sends no
`Authorization` header, and the model row is tagged `local`.

## The reasoning-template switch

This model thinks before it answers. `content` comes back **empty** when the output budget runs
out during that scratchpad, which is fatal to small requests: Rigel's title pass asks for 24
tokens. The door therefore sends

```json
"chat_template_kwargs": { "enable_thinking": false }
```

by default. Set `RIGEL_LLAMACPP_THINKING=1` to keep the scratchpad; Rigel renders
`reasoning_content` separately, and a reply that still ends with `finish_reason: "length"` and no
answer is reported as an error naming `max_tokens` rather than as an empty success.

## RAM and the visible mode

`-lm mmap+mlock` pins the 20.2 GiB model in RAM and is worth roughly double the throughput, but it
only works when the whole file fits in free memory. A start compares free physical RAM with the
GGUF size plus 512 MiB:

- enough room — `-lm mmap+mlock`, status mode `pinned`;
- not enough room — the flag is omitted, the server still starts, and status mode reads `mmap`
  with the free figure. The fallback is never silent.

## Failure messages

| Situation | What Rigel says |
|---|---|
| No profile or a missing file | names `RIGEL_LLAMACPP_SERVER` / `RIGEL_LLAMACPP_MODEL` and `rigel local setup` |
| Something else on the port | refuses to start a second server on that port |
| A healthy server is already there | attaches instead of starting another |
| Still loading after 120 s | says so, names the PID and the log, and leaves it running |
| Server exited during a turn | the last log lines, with the exit status |
| No answer, `finish_reason: length` | names `max_tokens` and the reasoning scratchpad |

Only one server should run: a second one halves usable RAM and exhausts the card. Stop by the
recorded PID (`rigel local stop`), never by process-name pattern.

## Environment

| Variable | Effect |
|---|---|
| `RIGEL_LLAMACPP_BASE_URL` | the door's base URL, when the server is elsewhere |
| `RIGEL_LLAMACPP_SERVER` | the `llama-server` binary |
| `RIGEL_LLAMACPP_MODEL` | the GGUF to serve |
| `RIGEL_LLAMACPP_THINKING` | `1` keeps the reasoning scratchpad (default: off) |

The port is part of the saved profile; `RIGEL_LLAMACPP_BASE_URL` wins over it for the door. Do not
expose the port publicly: `llama-server` has no key unless one is passed with `--api-key`.
