# AGENTS.md

Policy for AI coding agents (Cursor, Claude Code, Codex, etc.) operating in
this repository. Both Cursor and Claude Code automatically read this file
on session start; editing it takes effect immediately for the next session.

The rules below are the non-negotiable minimum. Individual contributors
running their own agents in this checkout are expected to enforce the same
rules even if their agent does not natively read `AGENTS.md`.

## Hard rules

1. **Never merge pull requests.** Agents open pull requests and then stop.
   Squash, rebase, merge-commit, auto-merge, or admin-override — all of
   them, via `gh pr merge`, the GitHub REST `PUT /repos/{o}/{r}/pulls/{n}/merge`
   endpoint, or any other surface — are out of scope. The human owner merges.
   This rule exists because the only gate between an in-progress agent task
   and a merge to `master` is the agent's own self-restraint; everything
   else (branch protection, CI gating) can be reconfigured by the same
   credential the agent is operating under. Handing merge control back to a
   human is the one guarantee an agent can still offer.

2. **Never push directly to `master`.** Work on a `DAY-XXX` branch cut from
   a just-pulled `master` and push to that branch only. The release
   workflow is the sole exception — it uses `github-actions[bot]` to push
   the `chore(release): vX.Y.Z` commit from inside CI, which is a
   workflow-driven push, not an agent-driven one.

3. **Never force-push to a shared branch.** `git push --force-with-lease`
   on the agent's own `DAY-XXX` feature branch is fine (the user's
   one-commit-per-branch convention requires it). Force-pushing `master`,
   a release branch, or any branch another agent or human is collaborating
   on is not.

4. **Never bypass or lower branch protection, CI requirements, or repo
   policy.** If a gate blocks a merge or push, surface it to the human
   and stop. Do not reach for `gh pr merge --admin`, do not toggle
   `enforce_admins` off, do not remove required status checks to unblock a
   red CI run, and do not disable the `check-semver-label` workflow to
   make a `semver:none` PR merge without the label.

5. **Never commit files that contain secrets.** `.env`, `credentials.json`,
   `*.p12`, private keys, and anything matching an obvious secret pattern
   stay out of commits. If the task genuinely needs a secret to be
   referenced, reference it by env-var name only and flag the requirement
   to the human.

## Working conventions

The following are the repo's existing working agreements. Agents must
follow them; humans editing directly are also expected to.

- **Branch naming:** `DAY-XXX`, where `DAY-XXX` is the GitHub issue number
  the work corresponds to. File an issue first (brief is fine) if none
  exists, then branch from a freshly-pulled `master`.

- **Commit message shape:**

  ```
  <branch>: <Title>

  <Paragraph describing what changed and why. Multiple paragraphs are
  allowed when the change has distinct pieces, but keep it prose — this
  is what shows up in the squash-merge commit, the release CHANGELOG when
  applicable, and `git log` archaeology years later.>
  ```

  Example: `DAY-149: Keep Dayseam scheduler running when the window is closed (Tier A)`.

- **One commit per branch.** When updating an in-flight PR, amend the
  existing commit and force-push the feature branch (`git commit --amend
  && git push --force-with-lease`). Change the commit message if the
  amended change is meaningfully different from the original scope.

- **Semver labeling:** every PR must carry exactly one of
  `semver:none` / `semver:patch` / `semver:minor` / `semver:major`. The
  `check-semver-label` workflow enforces this on the way in, and
  `release.yml` auto-cuts a release when a `semver:{patch,minor,major}`
  PR merges. Docs, CI, and chore PRs use `semver:none` so they do not
  trigger a release. See `.github/workflows/release.yml` for the full
  contract.

- **CHANGELOG discipline:** PRs that ship user-visible behaviour add an
  entry to the `[Unreleased]` section of `CHANGELOG.md`. The release
  workflow closes `[Unreleased]` into `[X.Y.Z] - YYYY-MM-DD`
  automatically as part of its `chore(release)` commit (DAY-155 wired
  `scripts/release/close-changelog.sh` into `release.yml`); agents
  must not pre-rename the block by hand unless they are deliberately
  using the capstone-PR pattern (pre-close for a reviewable diff),
  which the automation detects and skips. Before DAY-155 this was an
  unwritten convention that slipped twice in practice — the v0.7.0 →
  v0.8.0 pair (no `[0.7.0]` block exists on master at all) and the
  v0.8.1 → v0.8.2 pair (v0.8.2 re-shipped v0.8.1's DAY-161 entry on
  top of its own DAY-159 entry).

- **Verification before claiming done:** before opening a PR the agent
  should have run the relevant subset of `pnpm -r lint`, `pnpm -r
  typecheck`, `pnpm -r test`, `cargo fmt --all -- --check`, `cargo clippy
  --workspace --all-targets -- -D warnings`, and `cargo test --workspace`
  locally, and included the result in the PR description. "I ran the
  tests" without the evidence does not satisfy this.

## Useful pointers for a fresh agent

- **Product and architecture orientation:** `ARCHITECTURE.md` at the
  repo root is the canonical "how this app is built" doc; it is kept in
  sync as the source of truth for crate layout, IPC contracts, and
  background-execution policy.
- **Release mechanics:** `.github/workflows/release.yml` +
  `scripts/release/*.sh` (especially `bump-version.sh`,
  `extract-release-notes.sh`, and `generate-latest-json.sh`).
- **Branch protection setup:** `scripts/setup-branch-protection.sh` is
  the intended one-shot for applying `master` protection, but note the
  known issue tracked in the follow-up to issue #157 — it currently
  requires a team of at least two to merge PRs, so it has not yet been
  run against the solo `dayseam/dayseam` repo.
- **Review docs:** `docs/review/` contains the running archive of
  periodic holistic reviews, ordered by date. The most recent one is
  usually the best starting point for understanding current priorities.
