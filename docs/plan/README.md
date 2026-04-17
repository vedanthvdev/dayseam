# Dayseam implementation plans

The v0.1 design ([`docs/design/2026-04-17-v0.1-design.md`](../design/2026-04-17-v0.1-design.md)) decomposes into three sequential phase-plans. Each phase ends with **working, testable software** — a clean integration point where you could stop, demo, and come back later.

## Why three plans, not one

A single 20+ PR plan document is hard to review and navigate. Three phase-plans — each ~6–8 PRs — are the natural split because each ends at a genuine demo boundary:

| Phase | Plan document | What it produces |
|---|---|---|
| **1. Foundations** | [`2026-04-17-v0.1-phase-1-foundations.md`](./2026-04-17-v0.1-phase-1-foundations.md) | Mac app boots to an empty themed window. All SDK traits, database, secrets, event bus, error taxonomy, and CI pipeline are in place. No user-facing features yet. |
| **2. Local-git end-to-end** | `2026-XX-XX-v0.1-phase-2-local-git.md` *(written when Phase 1 lands)* | First genuinely usable slice: pick a date, generate a report from local git repositories, save it to a folder or Obsidian vault. The product becomes dogfoodable. |
| **3. GitLab + polish + release** | `2026-XX-XX-v0.1-phase-3-gitlab-release.md` *(written when Phase 2 lands)* | Shippable v0.1.0: GitLab connector added, per-source error cards, Playwright E2E happy path, codesigned + notarised `.dmg`, first tagged GitHub release. |

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

## Semver labels during pre-v0.1.0

Per the design doc, the repo starts at `VERSION=0.0.0` and "0.1.0 is reserved for the first feature-complete v0.1 release." While we are pre-0.1.0:

- **All Phase 1 and Phase 2 PRs carry `semver:none`.** Nothing in them is meant to ship standalone; we accumulate toward the first real release.
- **The final PR of Phase 3 — the "v0.1 feature-complete" capstone — carries `semver:minor`.** Applied to `VERSION=0.0.0` this bumps to `0.1.0` and triggers the first tagged release through the release workflow.
- **After 0.1.0 ships**, the label policy switches to the design doc's normal rules: every feature PR carries the `semver:*` label that matches its change, and each merge auto-releases.

## Non-goals of these plans

- **Not exhaustive line-level code.** The plan shows key interfaces, invariants, and test shapes. The executing engineer fills in straightforward glue (imports, boilerplate) without the plan narrating every line.
- **Not a replacement for the design doc.** The design explains *why*; the plan tells you *what files, in what order, proven by what tests*. If a plan step and the design conflict, the design is the contract — flag the conflict in the PR.
- **Not locked in stone.** If Phase 1 execution reveals a better decomposition, amend the plan document on the current PR before proceeding. Plans serve execution, not the other way around.
