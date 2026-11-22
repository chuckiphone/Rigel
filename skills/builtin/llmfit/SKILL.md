---
name: llmfit
description: Detect this machine's CPU, RAM, GPU and VRAM, then rank local models by whether they fit and how fast they will run.
license: MIT
metadata:
  author: Rigel
  tags:
    - llmfit
---

# llmfit

## Purpose


Parameter count is not a fit decision. llmfit probes the hardware — `nvidia-smi`,
`rocm-smi` or `system_profiler` — and scores models on quality, speed, fit and context for
the detected VRAM and backend.

## Install

```bash
winget install --exact --id AlexsJones.llmfit   # Windows
brew install AlexsJones/llmfit/llmfit           # macOS / Linux
cargo install llmfit                            # from source
llmfit --help
```

## Answer a real hardware question

1. Name the machine and card in the answer. "A 6 GB card" is not enough: the vendor, exact
   model and driver decide which detection path works.
2. Run `llmfit` and read the detected GPU line **before** reading the rankings. If it says
   no GPU, the table is a CPU answer.
3. Quote the quantizations that fit, the estimated tokens per second and the context length
   for each. Keep the numbers with the hardware row they came from.
4. Prefer a measured benchmark row over an estimate when the tool offers one. An estimate
   from another card is not a measurement of this one.

## Caveats to state

- Integrated GPUs share system RAM; subtracting VRAM twice produces a wrong recommendation.
- Multi-GPU totals are not one pool unless the runtime pools them.
- A model that fits with no context headroom will fail on long files. Say the headroom.
- macOS unified memory is one pool; do not compare it to a discrete card by VRAM alone.

## Boundaries

- Do not install a model or start a server without asking: downloads are large and the
  runtime may already be in use.
- Do not present a fit score as a quality score. It answers "will it run", not "is it good".
