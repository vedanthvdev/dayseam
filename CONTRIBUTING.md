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
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm -r lint
pnpm -r typecheck
pnpm -r test --if-present
```

`--all-features` is load-bearing: the Tauri desktop crate ships dev-only
commands behind the `dev-commands` feature, and running the tests without
it silently skips their coverage.

CI runs the same commands — if they are green locally they will be green in CI.

### Accepting `insta` snapshot changes

`dayseam-core` and `dayseam-report` use `insta` for golden snapshots
(error-code registry, rendered report drafts). When an intentional
change modifies a snapshot, run

```bash
cargo insta accept -p <crate>
```

to rewrite the `.snap` files on disk, review the diff in your PR, and
commit the updated snapshots alongside the change. Tests fail loudly
until the new snapshot is either accepted or the change is reverted —
never `INSTA_UPDATE=always` your way past a failure.

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

## Frontend conventions

- **React owns the DOM.** The only node created outside the React tree is the
  startup splash defined in [`apps/desktop/index.html`](./apps/desktop/index.html)
  and dismissed via [`apps/desktop/src/splash.ts`](./apps/desktop/src/splash.ts).
  It has to paint before any JS bundle loads, which is the sole justification —
  please don't copy the pattern for new UI. If you need pre-hydration state
  (another splash surface, a theme, an error shell), extend
  [`apps/desktop/public/hydrate-theme.js`](./apps/desktop/public/hydrate-theme.js)
  or add a sibling script there rather than reaching into `document` from a
  React component.
- **Theme parity.** The pre-paint hydration in
  [`apps/desktop/public/hydrate-theme.js`](./apps/desktop/public/hydrate-theme.js)
  and the React-side helpers in
  [`apps/desktop/src/theme/theme-logic.ts`](./apps/desktop/src/theme/theme-logic.ts)
  share the same storage key, resolution rules, and DOM write order by design —
  the parity test in
  [`src/__tests__/hydrate-theme.test.ts`](./apps/desktop/src/__tests__/hydrate-theme.test.ts)
  fails the suite if either half drifts. If you need to change theme
  behaviour, change both files together and let the parity test prove you got
  it right.

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
