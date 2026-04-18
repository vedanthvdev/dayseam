# Contributing to Dayseam

Thanks for taking the time to contribute. Dayseam is an early-stage
open-source project; the patterns below are what land cleanly today.

## Prerequisites

- **Rust 1.88+** matching the pin in [`rust-toolchain.toml`](./rust-toolchain.toml).
  Install via [`rustup`](https://rustup.rs/); `rustup` auto-detects the
  toolchain from the pin file on first `cargo` invocation in the repo.
- **Node.js 20+** and **pnpm 10+** (Corepack recommended:
  `corepack enable && corepack prepare pnpm@10.28.0 --activate`).
- **Xcode Command Line Tools** (`xcode-select --install`) on macOS.
- **GitHub CLI** (`gh`) if you want to open PRs from the terminal.

## Install and run

```bash
git clone https://github.com/vedanthvdev/dayseam.git
cd dayseam
pnpm install
cargo build --workspace
```

Launch the desktop app in dev mode:

```bash
pnpm tauri:dev
```

## Run the full test suite

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm -r lint
pnpm -r typecheck
pnpm -r test --if-present
```

CI runs the same commands — if they are green locally they will be green in CI.

## Branching and commits

- **Never commit directly to `master`.** Always open a PR.
- **Branch name:** `DAY-<issue-number>-<kebab-case-title>` — e.g.
  `DAY-42-gitlab-event-pagination`. The prefix ties the branch to a GitHub
  Issue.
- **One commit per branch.** If a reviewer asks for changes, amend the commit
  (`git commit --amend --no-edit`) and force-push with
  `git push --force-with-lease`. The PR history stays a single commit.
- **Commit message format:**

  ```
  <branch>: <Title>

  <One or two paragraphs describing what changed and why. No lists here —
  keep it readable as prose. Reference the issue in the PR body, not the
  commit message.>
  ```

- **PR title:** `DAY-<issue>: <Title>`.
- **Required label:** exactly one of `semver:major`, `semver:minor`,
  `semver:patch`, or `semver:none`. The `check-semver-label` workflow fails
  the PR until this is set.

## Before opening a PR

- [ ] Rebase onto an up-to-date `master`.
- [ ] Tests pass locally.
- [ ] The branch has a single commit following the message format above.
- [ ] The PR body references the issue it closes and the design/plan doc(s)
      it implements.

## Design and plan documents

Before you change something fundamental, read the top-down reference
document first:

- **Architecture & roadmap:** [`ARCHITECTURE.md`](./ARCHITECTURE.md) —
  the living, top-down view of the system: principles, repo layout,
  backend/frontend architecture, the connector and sink contracts, and
  the versioned roadmap. If your PR changes the shape of the system,
  update this file in the same PR.
- **Per-version design docs:** [`docs/design/`](./docs/design/) — deep
  detail for a specific release (schema, templates, error codes).
- **Per-phase implementation plans:** [`docs/plan/`](./docs/plan/) —
  the "what files in what order, proven by what tests" level.

Design docs describe *why*; plans describe *what files in what order,
proven by what tests*. If a plan step and the design conflict, the
design is the contract — flag the conflict in the PR.

## License

By contributing you agree your contribution is licensed under
[AGPL-3.0-only](./LICENSE).
