# Skill intelligence — scanning, safety, dedup, ranking

Companion to [phase-0-teardown.md](phase-0-teardown.md). This specifies the layer that makes the package manager worth using: not "here are 16,000 skills", but **"here is the one you want, here is what it will touch, and here is why it beats the nine copies."**

---

## 1. Why this is the real product

The teardown's gap table ranked "skill package manager" #1 on demand and low effort. The moment you actually crawl GitHub, a new problem appears that no incumbent solves: **the ecosystem is full of near-duplicates and unverifiable claims.**

Concrete evidence, from a controlled test rather than marketing:

| Skill | Advertised saving | Measured saving (JetBrains, 80 paired tasks) |
|---|---|---|
| caveman | −65% | **−8.5%** |
| rtk | −60–90% | **+7.6% (worse)** |
| ponytail | −54% code / −22% tokens / −20% cost | **−15% code / −10.3% cost / −11% time**, no detectable quality change |

Source: [JetBrains AI blog, 28 Jul 2026](https://blog.jetbrains.com/ai/2026/07/ponytail-skill-claude-tested/).

Two of three token-saver skills are wildly overclaimed; one is actively harmful. Star counts and install numbers do not surface that. **A catalogue that shows measured evidence instead of advertised evidence is the differentiator**, and it is a direct extension of the "accountability" thesis in the teardown.

---

## 2. Pipeline

Four stages. Stage 1 runs offline on a schedule; stages 2–4 run at ingest; the UI only ever reads the local index.

### Stage 1 — Discover

| Source | Method | Notes |
|---|---|---|
| GitHub code search | `filename:SKILL.md` | **Requires auth**, 10 requests/min. Budget for ~600/hr, not 5,000 |
| GitHub topic search | `topic:claude-skills`, `topic:agent-skills`, `topic:claude-code-skills`, `topic:ai-agents` | Cheap, high precision |
| Known aggregators | skills.sh counters, skills.browseract.com (16,494 skills / 239M installs), awesome-* lists | Seed only; never authoritative |
| Direct | Paste any GitHub URL / `owner/repo` | Always supported, bypasses the index |
| Installed locally | Walk `~/.claude/skills`, `~/.agents/skills`, `.claude/skills` … | Import what the user already has |

**Rate-limit strategy (this is where naive implementations die):**

- Core REST: **60 req/hr unauthenticated, 5,000/hr authenticated**. Never ship unauthenticated.
- Code search: **10 req/min**, requires auth, default branch only, files under 384 KB, 100 results/page.
- Contents API: max **1,000 files per directory** → do not use it to walk big repos.
- Git Trees API: recursive response caps at **100,000 entries / 7 MB**, and returns `truncated: true`. On truncation, re-fetch non-recursively and descend by subtree.
- One `SKILL.md` fetch = one API call. A 300-skill repo would burn 300 calls. **Fetch the tree once, then use raw.githubusercontent.com for file bodies** (no API quota) — reserve API calls for metadata.
- Persist ETags and `pushed_at`; skip unchanged repos entirely. A nightly re-crawl should touch almost nothing.

**Never call GitHub on a keystroke.** Search-as-you-type reads SQLite FTS5 over the local index; network fills in behind it.

### Stage 2 — Normalize and fingerprint

Parse frontmatter (`name`, `description`, optional `license`, `compatibility`, `metadata`, `allowed-tools`), then compute three independent signals:

1. **Content SimHash** (64-bit) over the body after stripping markdown syntax, code fences, comments and whitespace.
2. **MinHash** (k=128, 5-token shingles) → Jaccard estimate, robust to reordering and small edits.
3. **Tree signature** — hash of the sorted relative paths + extensions + sizes.

**Cluster rule:** same cluster if `SimHash hamming ≤ 3` **or** `MinHash Jaccard ≥ 0.85` **or** identical tree signature with ≥ 0.6 content Jaccard. Three independent signals stops both false merges (two unrelated skills that share a boilerplate README) and missed clones (same skill, reformatted).

### Stage 3 — Safety analysis (static, never executes anything)

Produce a **capability inventory** by scanning bundled scripts (`.sh`, `.py`, `.js`, `.ts`, `.ps1`) and the instruction body:

| Capability | Detection | Weight |
|---|---|---|
| **Network egress** | `curl`, `wget`, `fetch(`, `requests.`, `http`, package registries | high |
| **Writes outside project** | absolute paths, `~`, `/etc`, `$HOME`, env files, shell rc files | **critical** |
| **Process spawn / installs** | `exec`, `subprocess`, `npm i`, `pip install`, `brew`, `uvx` | high |
| **Secret access** | reads of `.env`, `*_API_KEY`, keychain, ssh keys, `gh auth token` | **critical** |
| **Destructive commands** | `rm -rf`, `git push --force`, `git reset --hard`, `DROP`, `> /dev/sd` | **critical** |
| **Privilege escalation** | `sudo`, `doas`, `pkexec`, admin shell | **critical** |

Then compare **declared vs. actual**: if frontmatter `allowed-tools` claims read-only but a bundled script calls `curl`, that mismatch is surfaced as the headline warning, not buried.

Grade:

- **A** — no scripts, or read-only scripts; declared tools match; reputable provenance; license present.
- **B** — scripts present, capabilities match declared tools, no critical flags.
- **C** — writes inside project or installs packages; no critical flags.
- **D** — critical capability flagged (writes outside project, secret access, destructive command).
- **F** — critical flag plus no license, no provenance, or a declared/actual mismatch.

**Wording rule:** never say "safe". Say **"no dangerous capabilities detected in static review"**, and always show the exact evidence lines that were scanned with file and line number. Pair it with: "static review cannot prove a skill is harmless; sandbox on first run."

### Stage 4 — Rank, dedupe, pick a winner

Canonical selection score (0–100):

| Signal | Weight | Why |
|---|---|---|
| Maintenance recency (last commit / release) | 25 | Abandoned copies are the main failure mode |
| Safety grade (A=100 … F=0) | 20 | Non-negotiable |
| Install count (skills.sh counter) | 15 | Weak alone, useful as a tiebreak |
| Stars / forks | 10 | Popularity ≠ quality, so capped |
| Completeness (has `references/`, `scripts/`, version, license, precise `description`) | 15 | A `description` under ~40 chars is why skills silently fail to trigger |
| Benchmark evidence (measured, third-party) | 10 | The differentiator |
| Author/provenance verification | 5 | Verified org, signed commits |

**UI behaviour:** one card per cluster. Under it: *"4 near-identical variants · showing the best maintained"* with a disclosure listing the others, their deltas ("same body, older name", "fork, 8 months stale", "identical content, no license"). Never hide the duplicates — hiding them makes the ranking unfalsifiable.

**Claim auditing:** if a skill's description advertises a percentage without a source, tag it **"unsubstantiated"**. If a third-party benchmark exists, show **measured** next to **advertised**, always in that order. This single UI element is what the teardown says nobody does.

---

## 3. Seed catalogue (first-run experience)

Ship a small, curated set so the app is useful before the user searches. Two are specified by request:

### ECC — `affaan-m/ECC` · a *bundle*, not a skill

- Positioning: "the operating layer for agent harnesses — skills, memory, planning and security."
- Scale: **311 skills** plus agents, hooks and workflow tooling; ships `AgentShield` (security component).
- Distribution: `npx ecc-universal install --guided`; native plugin for Claude Code and Codex, managed adapters for Cursor / OpenCode / Kimi Code, GitHub App.
- Traction (as of 20 Oct 2026): **1.2M skill installs on skills.sh**, 7,772 GitHub App installs, 52,252 `ecc-universal` npm downloads/last 30 days.

**Why it matters for our design:** ECC is the strongest evidence that the *generic package + manifest* model is correct — it is a bundle containing dozens of packages across six harnesses. It is also a partial competitor: it installs across tools, but from a CLI, with no browsable catalogue, no per-project enablement, no pre-install capability review and no duplicate handling. **Adopt it as a seed entry; compete on the layer above it.**

### Ponytail — a single *token-saving* skill

- Claim: makes agents write less code; advertised −54% code, −22% tokens, −20% cost, −27% time.
- Measured (JetBrains, 80 paired tasks): **−15% code, −10.3% cost, −11% time**, no detectable quality difference; the code reduction only appears where there was room to over-build.
- Companion entries: `ponytail-debt` (marks deliberate shortcuts with an upgrade path).

**Display rule:** show ponytail's measured numbers, not its advertised ones, and note the qualifier ("savings appear on tasks with room to over-build"). This is the app's proof-of-concept for evidence-first ranking — the first seed entry where the *honest* number is the headline.

> Do not silently bundle either. Both appear as curated entries in the Discover view, badged **Curated**, installable in one click, and removable like anything else. Nobody's machine gets modified on first launch.

### Bundled skills and the quality gate

Ten skills now ship inside the binary as reviewed `SKILL.md` bodies, indexed as
`builtin:<slug>` at window launch: SkillEvaluator, Headroom, AG-UI, mcptoon, llmfit, Semble,
Cognee (evaluate only), Blender MCP, Superpowers and Untangle Codebase. Indexing writes only to
the local shelf. Copying them into `~/.agents/skills` and `~/.claude/skills` is a separate
`rigel skills builtin --install`.

NVIDIA **SkillEvaluator** (Apache-2.0) was installed and run on the folder on 22 Nov 2026:

```bash
for dir in skills/builtin/*/; do skillevaluator quality-check "$dir"; done
```

The original eight pass the required check with grade A, between **90.0 and 96.2 / 100**.
Superpowers and Untangle Codebase were added after that sweep and follow the same frontmatter
and body rules. The findings left are advisory: some descriptions could name when *not* to use
the skill, and some entries lack a troubleshooting section or worked example. Two licence notes
in the original shortlist were corrected by the pinned repositories: Headroom is Apache-2.0,
not unverified.

The static check is a floor, not a certificate. It does not execute a skill's commands, and the
bundled bodies tell the agent to check a CLI's `--help` rather than trust a remembered flag.

### Pre-download gate: NVIDIA SkillSpector

Every third-party acquisition path now goes through NVIDIA
[SkillSpector](https://github.com/NVIDIA/SkillSpector): `SkillIndex::index_repo`, `index_skill`
(the skills.sh importer), the launch and five-minute source refresh, Settings → Skill shelf, and
`rigel skills index`. A single-skill URL is preflighted before Rigel fetches it. A repository
collection is preflighted before the walk too; because one aggregate verdict cannot tell two
independent skills apart, Rigel then writes the exact fetched bodies into one disposable
collection, scans it recursively, drops only the skills SkillSpector blocks, and ingests the
rest. Nothing is written to the index or a blob store before that per-skill scan. Installation
is gated again at `SkillIndex::install`, so a body indexed before the gate existed — or one
copied into the index by a direct ingest — is still scanned before any harness store is touched.
The install gate scans the exact `SKILL.md` bytes in a disposable directory and removes it
before writing.

The scanner runs its static, local pass:

```bash
skillspector scan <repo-or-directory> --no-llm --format json
```

`--no-llm` is deliberate: no skill body, repository content or prompt goes to a model provider,
and no provider key is needed. The gate is fail-closed. A missing executable, a non-JSON report,
a missing score or recommendation, a scanner error exit, omitted recursive results, a scanner
report with incomplete coverage, `risk_assessment.score > 50`, a `HIGH`/`CRITICAL` severity, or
`DO_NOT_INSTALL` stops or filters the operation. `CAUTION` is allowed and carried in the ingest
or install receipt so the shelf can show it; `SAFE` is not described as a guarantee. A blocked
single-skill target creates no source row and downloads no body. A blocked skill inside a
collection writes no row or blob, is named on stderr and counted in the ingest receipt's
`blocked` field, while safe siblings still index. A blocked install creates no harness directory.

The preflight makes SkillSpector clone or read the target once; the exact-body pass is what
enforces collections. This means a collection's `SKILL.md` bytes reach memory before their scan,
but never SQLite, the blob directory or a harness store. Scanning every remote subdirectory
separately before the walk would clone the whole repository once per skill, which is why the
collection pass is one local recursive invocation. Rigel downloads and installs only
`SKILL.md`, not a repository's bundled scripts, so those bodies are the installed artifact the
collection pass can prove. A single-skill URL preflight also sees that directory's scripts; a
collection's aggregate preflight sees the whole repository but is advisory, not the per-skill
gate.

Install the scanner once (Python 3.12+ and uv):

```bash
uv tool install git+https://github.com/NVIDIA/SkillSpector.git
```

If its shim is not on the app's PATH, set `RIGEL_SKILLSPECTOR_BIN` to the executable. Rigel
does not install the Python tool for you. SkillSpector is Apache-2.0; its own dependency and
vulnerability lookups are outside Rigel's network policy, and the gate is a static floor rather
than a sandbox.

On 24 Nov 2026, SkillSpector 2.13.7 scanned the ten bundled bodies through this path. Nine were
`SAFE` (0–7/100); `superpowers` was `CAUTION` at 45/100 with three findings. The policy above
allowed the caution verdict and the install receipt showed it; nothing was silently relabelled
as safe. Those are scanner findings, not a claim that the built-in review was wrong.

---

### Publisher signatures at load

Roadmap 0.3 item 77 closes the gap between "the body was scanned" and "the body the agent reads
is the body that was published." The check runs in `crates/rigel-core/src/skills/verify.rs` on the
exact string returned by `skill_load` and `rigel_skill_load`.

A signed `SKILL.md` adds three frontmatter fields:

```yaml
publisher: acme
content-sha256: <sha256 of the instructions after the frontmatter, lowercase hex>
signature: <ed25519 signature, lowercase hex>
```

The signature covers exactly:

```text
rigel-skill-v1
<skill id>
<publisher>
<content-sha256>
```

Including the skill id prevents a valid signature from being transplanted to another row. The
signature covers the instructions, not the metadata header; the instructions are what the agent
receives. A body that changes after signing fails the hash check, and a changed hash or publisher
fails the signature check.

Trust is project-local in `.rigel/skill-trust.json`:

```json
{
  "publishers": {
    "acme": "<32-byte ed25519 public key, lowercase hex>"
  }
}
```

A missing file trusts no publisher and accepts no unsigned downloaded skill. To accept unsigned
skills in one project, add the explicit flag:

```json
{
  "allow_unsigned": true,
  "publishers": {}
}
```

Built-in skills are compiled into the binary and indexed as `builtin:<slug>`; they do not need a
publisher signature because their origin is the build. A downloaded row cannot acquire that trust
by naming itself `builtin:`: the index source must also be `builtin`. A malformed trust file fails
closed for every downloaded skill. A signature block with only some of the three fields is an
error, not an unsigned skill.

**Recovery limit:** this is an authenticity check, not a sandbox or a revocation service. A
trusted publisher can sign a new malicious body, a stolen publisher key remains trusted until its
key is removed from the project file, and there is no transparency log or online revocation in
this pass. Unsigned mode is deliberately visible in the load result (`verification.status` is
`unsigned`), and the existing SkillSpector and script-approval gates still apply.

## 4. How this changes the ranked feature list

Append to §6 of the teardown:

| # | Feature | Impact | Effort | Notes |
|---|---|:--:|:--:|---|
| 11 | **Clone detection + canonical pick** — cluster near-duplicates, show the best maintained, disclose the rest | 4 | 3 | Cheap after fingerprinting; nobody does it |
| 12 | **Evidence-first ranking** — measured vs. advertised savings, "unsubstantiated" tags | 4 | 2 | Highest credibility-per-hour feature in the entire build |

*11 and 12 slot in directly behind #1, because both are only visible once the package manager exists.*

---

## 5. Storage sketch

```sql
-- package: one installable thing
CREATE TABLE package (
  id TEXT PRIMARY KEY,            -- owner/repo@path
  kind TEXT NOT NULL,             -- skill | mcp | prompt_pack | plugin_bundle
  name TEXT NOT NULL,
  description TEXT,
  manifest_json TEXT NOT NULL,
  tree_sig TEXT, content_simhash INTEGER, minhash BLOB,
  safety_grade TEXT, capabilities_json TEXT,
  canonical_of TEXT REFERENCES package(id),
  cluster_id TEXT,
  stars INTEGER, installs INTEGER, last_commit TEXT, license TEXT,
  fetched_at TEXT, etag TEXT
);

CREATE VIRTUAL TABLE package_fts USING fts5(name, description, body, content='package');
```

The whole point: **`canonical_of` and `cluster_id` make dedup a first-class property**, not a UI afterthought.

---

## 6. Open questions before implementation

1. **Repo scale.** Crawling every public `SKILL.md` is not feasible on 5,000 req/hr. Decide: nightly full crawl of a tracked repo list (~2–5k repos), or on-demand fetch with a thin cache? *Recommendation: tracked list + on-demand.*
2. **Fingerprint threshold tuning.** The 0.85 Jaccard / hamming ≤ 3 pair is a starting guess and must be validated against a hand-labelled sample of ~200 packages.
3. **Benchmark sourcing.** Only JetBrains is publishing controlled measurements. Without more, "evidence-first" applies to a small set. Decide whether to run our own micro-benchmarks (credible, expensive) or curate third-party ones (cheap, thin).
4. **Legal.** ECC is a third-party bundle with its own license; mirroring its 311 skills into our index needs a license check per package, not per repo.
