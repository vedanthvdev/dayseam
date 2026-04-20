# Dayseam implementation plans

The v0.1 design ([`docs/design/2026-04-17-v0.1-design.md`](../design/2026-04-17-v0.1-design.md)) decomposes into three sequential phase-plans. Each phase ends with **working, testable software** — a clean integration point where you could stop, demo, and come back later.

## Why three plans, not one

A single 20+ PR plan document is hard to review and navigate. Three phase-plans — each ~6–8 PRs — are the natural split because each ends at a genuine demo boundary:

| Phase | Plan document | What it produces |
|---|---|---|
| **1. Foundations** | [`2026-04-17-v0.1-phase-1-foundations.md`](./2026-04-17-v0.1-phase-1-foundations.md) | Mac app boots to an empty themed window. All SDK traits, database, secrets, event bus, error taxonomy, and CI pipeline are in place. No user-facing features yet. |
| **2. Local-git end-to-end** | [`2026-04-18-v0.1-phase-2-local-git.md`](./2026-04-18-v0.1-phase-2-local-git.md) | First genuinely usable slice: pick a date, generate a report from local git repositories, save it to a folder or Obsidian vault. The product becomes dogfoodable. |
| **3. GitLab + polish + release** | [`2026-04-20-v0.1-phase-3-gitlab-release.md`](./2026-04-20-v0.1-phase-3-gitlab-release.md) | Shippable v0.1.0: GitLab connector added, per-source error cards, Playwright E2E happy path, downloadable `.dmg` (unsigned for v0.1.0; codesign + notarisation tracked as a Phase 3.5 / v0.1.1 follow-up per the user decision recorded with the plan), first tagged GitHub release. |

## Why write them sequentially

Writing Phase 2 before Phase 1 has landed is speculative — the concrete shape of the SDKs and the Tauri IPC surface is best informed by having written them. So Phase 2's plan is drafted only after Phase 1 merges, and Phase 3's only after Phase 2 merges. This mirrors how we reviewed the design section-by-section rather than in one sitting.

## How each plan is executed

1. The plan document lists PRs as numbered tasks.
2. For each task:
   - Open a child Issue titled "DAY-N: &lt;task title&gt;".
   - Branch from freshly-pulled `master` as `DAY-N-<kebab-title>`.
   - Implement per the plan's steps.
   - Follow the project's one-commit-per-branch rule: the **first** change to the branch is a real `git commit`; **every subsequent change** is `git commit --amend` + `git push --force-with-lease`. Steps in the plan that say "Commit" after the first mean "amend and force-push."
   - Push, open PR, apply the task's designated `semver:*` label, request review.
   - Merge by squash when green and approved.

## Every phase ends with a review task

The **last task in every phase plan** is a cross-cutting hardening + review pass — not just "run the full test suite" but a deliberate, multi-persona deep review of the cumulative diff landed during the phase, looking for the class of bugs that only shows up when the pieces are seen together (cycles, leaked secrets, capability gaps, error-code overlaps, migration ordering, feature-flag regressions, drift between `ARCHITECTURE.md` and the shipped code).

The canonical shape of this task — inventory, multi-lens review, hardening battery, smoke test, enumerated findings with resolutions, review artifact under `docs/review/phase-N-review.md`, changelog entry — is documented once in **Phase 1's Task 10** ([`2026-04-17-v0.1-phase-1-foundations.md`](./2026-04-17-v0.1-phase-1-foundations.md#task-10-phase-1-hardening--cross-cutting-review)). Each subsequent phase plan ends with its own "Phase N hardening + cross-cutting review" task that reuses that shape, tuned to the phase's domain (e.g. Phase 2 adds a "identity linking regression" lens; Phase 3 adds a "release engineering + signed artifact integrity" lens).

A phase is not *done* when its feature tasks are merged. A phase is done when its review task is merged, every finding is resolved or explicitly deferred with a linked issue, and the hardening battery is green on `master`. Only then does the next phase's plan get drafted.

## Semver labels during pre-v0.1.0

Per the design doc, the repo starts at `VERSION=0.0.0` and "0.1.0 is reserved for the first feature-complete v0.1 release." While we are pre-0.1.0:

- **All Phase 1 and Phase 2 PRs carry `semver:none`.** Nothing in them is meant to ship standalone; we accumulate toward the first real release.
- **The final PR of Phase 3 — the "v0.1 feature-complete" capstone — carries `semver:minor`.** Applied to `VERSION=0.0.0` this bumps to `0.1.0` and triggers the first tagged release through the release workflow.
- **After 0.1.0 ships**, the label policy switches to the design doc's normal rules: every feature PR carries the `semver:*` label that matches its change, and each merge auto-releases.

## Deferred to Phase 3: graphify indexing for AI agents

> **Resolved 2026-04-20 (Phase 3 Task 7 / DAY-60):** scored against the
> three-axis rubric below on then-current `master` and resolved as
> **defer to v0.2**. Full scoring, repo-shape signal, and the five
> re-evaluation triggers live in
> [`docs/decisions/2026-04-20-graphify-deferred.md`](../decisions/2026-04-20-graphify-deferred.md);
> v0.2 follow-up is tracked in
> [#61](https://github.com/vedanthvdev/dayseam/issues/61). The
> original framing is preserved below for historical context.

A collaborator flagged [`safishamsi/graphify`](https://github.com/safishamsi/graphify) as a tool that can turn the repo into a queryable knowledge graph for AI coding agents working on Dayseam. We are **not** wiring it into Phase 1 or Phase 2, for three reasons:

- **Too early to be useful.** Through Phase 2 the workspace is small enough that `rg`, `cargo tree`, and `cargo doc` are faster and more accurate than any generated graph. A stale graph is actively worse than no graph, and nothing in the current toolchain keeps `graphify-out/` refreshed automatically.
- **Changes the trust surface.** `graphify` reads the whole codebase and writes summarised artifacts back into the repo. Before we commit any generated summaries we want an explicit rule about (a) what gets checked in vs `.gitignore`d, (b) who / what regenerates it, (c) whether secrets-adjacent files (`.env`, keychain helpers, signing scripts) are excluded. That rule belongs with the rest of the Phase 3 release-engineering work, not scattered across earlier PRs.
- **It should not become a second source of truth.** Our canonical description of the architecture is [`ARCHITECTURE.md`](../../ARCHITECTURE.md) plus the phase plans. Any `graphify` index is an *aid* to navigating them, not a replacement.

Phase 3's plan (written once Phase 2 lands) therefore owns the decision of whether to adopt `graphify`, under a dedicated task roughly shaped like:

- Evaluate `graphify` against the equivalent built-in tooling (`cargo doc --workspace`, `rust-analyzer`'s symbol index, the existing architecture doc) on a real Dayseam refactor task.
- If adopted, add a `docs/graphify/` refresh script and a CI guard that fails if the committed index is staler than the last `master` commit, so an out-of-date graph can never mislead an agent.
- Update `ARCHITECTURE.md` and `CONTRIBUTING.md` to tell human and AI contributors when to consult the graph vs the source-of-truth docs, and under what circumstances they must regenerate it.
- Decide on retention: whether the generated artifacts are checked in (reviewable, but adds repo weight) or `.gitignore`d with a one-command regenerator (lean, but no review trail).

Phase 3's review task must either land this decision or explicitly defer it with a tracked issue.

## Non-goals of these plans

- **Not exhaustive line-level code.** The plan shows key interfaces, invariants, and test shapes. The executing engineer fills in straightforward glue (imports, boilerplate) without the plan narrating every line.
- **Not a replacement for the design doc.** The design explains *why*; the plan tells you *what files, in what order, proven by what tests*. If a plan step and the design conflict, the design is the contract — flag the conflict in the PR.
- **Not locked in stone.** If Phase 1 execution reveals a better decomposition, amend the plan document on the current PR before proceeding. Plans serve execution, not the other way around.
