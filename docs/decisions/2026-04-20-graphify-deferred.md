# Decision: Defer `graphify` adoption to post-v0.1

- **Date:** 2026-04-20
- **Status:** Decided — defer
- **Owners:** Dayseam maintainers
- **Supersedes:** N/A (first entry in `docs/decisions/`)
- **Tracking issue for re-evaluation:** [#61](https://github.com/vedanthvdev/dayseam/issues/61) (v0.2 follow-up).

## Context

[`docs/plan/README.md`](../plan/README.md) flagged
[`safishamsi/graphify`](https://github.com/safishamsi/graphify) as a tool
that can turn the repo into a queryable knowledge graph for AI coding
agents working on Dayseam, and explicitly reserved the adopt-or-defer
decision for a dedicated Phase 3 task. That task is Task 7 of
[`docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md`](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md):
> "Phase 3's review task must either land this decision or explicitly
> defer it with a tracked issue."

This document is that decision.

## What `graphify` is, in one paragraph

`graphify` ingests a folder (code, docs, papers, images, transcripts),
runs an AST pass over the code, dispatches LLM subagents over the rest,
and emits three artefacts: an interactive `graph.html`, a
GraphRAG-ready `graph.json`, and a plain-language `GRAPH_REPORT.md`
with community detection, "god nodes," and "surprising connections."
Edges are tagged `EXTRACTED` (structural), `INFERRED` (LLM judgement),
or `AMBIGUOUS`. Staleness is managed through a post-commit hook
(AST-only re-extract on code changes, no LLM), a `--watch` loop (same
shape), and a manual `--update` path for docs/papers/images (which
does cost LLM tokens).

## The decision

**Defer adoption to post-v0.1.** Keep the question open as a v0.2
follow-up that re-evaluates against a larger codebase and a larger
contributor pool. No `graphify-out/` artefacts are committed to the
repo. No `.github/workflows/graphify-freshness.yml` lands in this PR.
No `scripts/graphify/refresh.sh` lands in this PR. `ARCHITECTURE.md`
and `CONTRIBUTING.md` are not updated.

## Evaluation

The plan prescribed a three-axis scoring rubric and said: **adopt if
two of three axes come up positive; otherwise defer.** Here are the
three axes scored against today's `master`.

### Repo shape on evaluation day

These numbers are the *input* to the scoring; captured so that the
re-evaluation trigger is comparable.

| Signal | Value on 2026-04-20 `master` |
|---|---|
| Workspace crates | 12 (listed in `Cargo.toml` `[workspace].members`) |
| Tracked Rust + TS/TSX source files | 294 |
| Rough total LOC (Rust + TS/TSX) | ~37 800 |
| Canonical architecture docs (design + plan + review + ARCHITECTURE.md) | ~6 000 lines |
| Active committers since Phase 1 opened | 2 |
| Long-lived branches | 1 (`master`); feature branches live < 1 week |
| E2E coverage of the click path | Green (Task 5 / DAY-58 BDD suite) |
| `docs/design/` cross-reference density | Hand-curated; design §6.2, §10.3, §13, §17 all cross-link explicitly |

### Axis (a) — does it surface things the existing toolchain does not?

**Existing toolchain**: `cargo doc --workspace` (Rust symbol graph),
`rust-analyzer` (IDE-level cross-references), `rg` (textual grep),
`cargo tree` (dep graph), `packages/ipc-types/` (the TS↔Rust IPC
contract, machine-generated from `ts-rs`), and the written docs above.

**What `graphify` adds on top**: LLM-inferred
`semantically_similar_to` and `rationale_for` edges, plus community
detection across the union of code + docs.

**What we find when we apply that to this repo**:

- The design doc already *is* the cross-reference graph. §6.2
  (GitLab connector) explicitly points at §10.3 (cross-source
  rollup), §13 (security posture), and §17 (release engineering); the
  phase plans then point back at the design sections. The inferred
  "surprise connections" graphify would find are, by construction,
  already written down as explicit cross-references.
- `rationale_for` is interesting in theory, but our rationale is
  *already* concentrated in the design doc and phase-plan "Why next"
  / "Why first" headers. A `rationale_for` edge between a node in
  `connector-gitlab/src/auth.rs` and a paragraph in `design §13`
  would duplicate a relationship the file-level review comments in
  `phase-3-review.md` (Task 8) will re-validate by hand anyway.
- For the TS side, the `@dayseam/ipc-types` package *is* the
  cross-boundary contract; there is nothing a Python tool could
  extract that the generated types don't already guarantee.

**Verdict on (a): marginal positive at best; nothing that the existing
toolchain cannot already surface in under a minute of `rg` or
`cargo doc`.** In a larger repo with looser cross-referencing
discipline this would flip; today it does not.

### Axis (b) — is the staleness signal trustworthy?

`graphify` handles staleness two ways:

1. **Code changes**: post-commit hook / `--watch` loop re-runs
   AST extraction only. Fast, deterministic, no LLM tokens.
2. **Doc, paper, image, fixture changes**: requires a manual
   `graphify --update` that dispatches semantic subagents (LLM-backed)
   over the changed files.

**Why this is a problem on Dayseam specifically**: the project's
*canonical* architecture source is the set of markdown documents
under `docs/design/`, `docs/plan/`, `docs/review/`, and
`ARCHITECTURE.md` — ~6 000 lines today, and Phase 3 alone added
~1 200 more. Every phase review updates those docs. A Dayseam
`master` that is one commit old is therefore *guaranteed* to have
a stale semantic index unless a human (or a scheduled job) runs
`graphify --update` after every doc-touching merge.

The `plan/README.md` Phase 3 deferral explicitly called this out:

> "Too early to be useful. A stale graph is actively worse than no
> graph, and nothing in the current toolchain keeps `graphify-out/`
> refreshed automatically."

The tool's own post-commit hook does not cover the doc side; we would
have to build the freshness CI guard the plan described, and that
guard would either:

- require the CI runner to spend LLM tokens on every merge that
  touches a markdown file (cost we do not want to opt into for v0.1),
- or accept staleness on the doc side and document the staleness as
  a known limitation (which defeats the value proposition of a
  graph the reviewer can trust).

**Verdict on (b): negative.** The freshness signal is trustworthy for
the half of the repo that needs it least (AST over code, where the
compiler and `cargo doc` already tell the truth) and untrustworthy
for the half that would actually benefit from indexing (the prose
docs that codify the architecture).

### Axis (c) — what happens to the trust surface when generated artefacts get checked in?

The `plan/README.md` defer listed three concerns; each one still
applies today and the answers are not better than they were when
Phase 1 opened:

1. **What gets checked in vs `.gitignore`d?** Committing
   `graphify-out/graph.html`, `graph.json`, and `GRAPH_REPORT.md`
   puts ~hundreds of KB to a few MB of machine-generated text into
   every PR that runs the refresh, which bloats `git log -p` and
   `git blame` on human-authored files nearby. The lean alternative
   — `.gitignore` the output and regenerate on demand — removes
   the review trail the plan explicitly wanted.
2. **Who regenerates it?** A single-contributor project has no
   natural owner besides the committing engineer; that engineer
   already has `cargo doc` and `rg` and does not need a second tool.
3. **Secrets-adjacent files.** Dayseam carries several categories of
   file that must never land in an LLM-ingested artefact:
   - `connector-gitlab/tests/fixtures/**` — recorded GitLab API
     payloads. The fixtures are redacted, but we do not want an LLM
     paraphrasing them into a `GRAPH_REPORT.md` that a future
     reader then treats as canonical.
   - `apps/desktop/src-tauri/capabilities/*.json` — exact allowlist
     strings the Phase 1 capability-parity guard depends on. A
     paraphrased summary is strictly worse than reading the file.
   - `scripts/release/*.sh` and `.github/workflows/release.yml` —
     the release tooling is short; a summary of it is noise.
   - `dayseam-secrets` Rust source and its call sites — an
     LLM-authored summary here would be a net-negative security
     artefact (one more surface for a reader to trust by mistake).

   `graphify` supports exclusion via file-detection rules, but
   per-project tuning is a maintenance burden that buys us nothing
   unless (a) and (b) are already strongly positive. They are not.

**Verdict on (c): negative.** Adopting graphify adds review and
security surface without a compensating reduction elsewhere.

### Score summary

| Axis | Score |
|---|---|
| (a) surfaces things `cargo doc` + `rg` + design docs don't | **marginal** |
| (b) staleness signal is trustworthy on this repo | **negative** |
| (c) trust surface is acceptable when artefacts are committed | **negative** |

0 out of 3 axes come up clearly positive. The plan's rubric
("adopt if 2 of 3 are positive") therefore resolves to **defer**.

## What would flip the decision

Re-open the question when **any** of these become true:

1. **Workspace size roughly doubles.** > ~25 crates or > ~600
   source files, such that a new contributor's first-week "where
   does X live" questions stop being answerable in < 60 s with
   `rg` + `cargo doc`.
2. **Contributor count crosses ~3 concurrent.** More heads means
   more divergent mental models, which is where a shared graph
   earns its keep. The same logic applies to AI agents working in
   parallel on the same repo; if parallel agent sessions become a
   regular pattern rather than an occasional one, re-evaluate.
3. **Architecture docs start drifting from code.** If a review
   surfaces that the design doc and shipped behaviour disagree and
   no human noticed, the implicit assumption behind deferring (that
   the hand-curated docs *are* the graph) is no longer true. A
   mechanical graph then starts paying for itself.
4. **LLM-agent orchestration becomes a first-class development
   surface.** If the project adds its own AI-agent tooling that
   would consume a graph at inference time (e.g. an in-app
   "suggest next step" that reads repo structure), the trust-surface
   calculus changes because the consumer is already a machine.
5. **A comparable zero-LLM-cost alternative ships.** If a pure-AST
   or `rust-analyzer`-backed tool emerges that gives the same
   cross-domain (code + docs) graph without per-regen token cost,
   the (b) axis flips to positive and the decision re-scores.

## What happens in the meantime

- No new files land for this adoption attempt. The repo continues to
  use `cargo doc --workspace`, `rust-analyzer`, `rg`, the design and
  plan documents, and `packages/ipc-types/` as its knowledge graph.
- The v0.2 issue linked below carries this document as its living
  spec. When any of the five trigger conditions above fires, a new
  task opens under the v0.2 plan and supersedes this decision.
- If a contributor wants to run `graphify` locally against their
  checkout for their own use, that remains entirely supported — the
  tool is a global CLI, not a repo dependency. Nothing about this
  decision blocks personal exploration; it only blocks *committing*
  generated artefacts into `master`.

## Links

- Originating deferral: [`docs/plan/README.md` — "Deferred to Phase 3: graphify indexing for AI agents"](../plan/README.md#deferred-to-phase-3-graphify-indexing-for-ai-agents)
- Plan task: [Phase 3 Task 7](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-7-graphify-adopt-or-defer-decision)
- Tool: [`safishamsi/graphify`](https://github.com/safishamsi/graphify)
- Re-evaluation tracking issue: [#61 — v0.2: Re-evaluate graphify knowledge-graph adoption](https://github.com/vedanthvdev/dayseam/issues/61).
