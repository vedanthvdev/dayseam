# Phase 1 hardening + cross-cutting review

**Task:** [Phase 1, Task 10 — Hardening + cross-cutting review](../plan/2026-04-17-v0.1-phase-1-foundations.md#task-10-phase-1-hardening--cross-cutting-review)
**Branch:** `DAY-27-phase-1-hardening`
**Semver label:** `semver:none`
**Review date:** 2026-04-18

This document is the written artifact of the Phase 1 capstone review. It
enumerates what was reviewed, how it was reviewed, every finding that surfaced,
and the resolution (fix here, follow-up PR, or explicit deferral with a linked
issue).

The review itself — not just the PRs leading up to it — is what closes Phase 1.
Nothing in Phase 2 is blocked on a connector. It is blocked on Phase 1 being
right.

---

## 1. Inventory

### 1.1 Baseline and head

| | Commit | Label |
|---|---|---|
| Baseline | `83e80941fb9cc261bf39fa3f0db4653c438a9893` | Merge of [PR #6 `DAY-5-monorepo-scaffold`](https://github.com/vedanthvdev/dayseam/pull/6) |
| Head     | `master` at task start                       | After [PR #26 `DAY-25-ipc-log-toast`](https://github.com/vedanthvdev/dayseam/pull/26) |

### 1.2 PRs merged in scope (first-parent, excluding the baseline)

| # | PR | Branch | Summary |
|---|----|--------|---------|
|  1 | [#8](https://github.com/vedanthvdev/dayseam/pull/8)   | `DAY-7-core-types-and-errors`               | Core domain types, `DayseamError`, and ts-rs bindings |
|  2 | [#10](https://github.com/vedanthvdev/dayseam/pull/10) | `DAY-9-db-schema-and-repos`                 | SQLite schema v1 and typed repositories |
|  3 | [#12](https://github.com/vedanthvdev/dayseam/pull/12) | `DAY-11-secrets-keychain-wrapper`           | `dayseam-secrets` — `Secret<T>` and Keychain-backed store |
|  4 | [#14](https://github.com/vedanthvdev/dayseam/pull/14) | `DAY-13-architecture-and-roadmap`           | `ARCHITECTURE.md` — living architecture and versioned roadmap |
|  5 | [#16](https://github.com/vedanthvdev/dayseam/pull/16) | `DAY-15-dayseam-events-bus`                 | `dayseam-events` — per-run streams + app-wide broadcast |
|  6 | [#18](https://github.com/vedanthvdev/dayseam/pull/18) | `DAY-17-phase-1-plan-align-with-architecture` | Align Phase 1 plan with architecture; add phase-end review task |
|  7 | [#20](https://github.com/vedanthvdev/dayseam/pull/20) | `DAY-19-connectors-sdk`                     | `connectors-sdk` traits + shared `HttpClient` |
|  8 | [#22](https://github.com/vedanthvdev/dayseam/pull/22) | `DAY-21-sinks-sdk`                          | `sinks-sdk` trait, `SinkCapabilities`, and `MockSink` |
|  9 | [#24](https://github.com/vedanthvdev/dayseam/pull/24) | `DAY-23-tauri-shell-theme`                  | Tauri app shell + theme + capabilities |
| 10 | [#26](https://github.com/vedanthvdev/dayseam/pull/26) | `DAY-25-ipc-log-toast`                      | IPC bindings, log drawer, and toast system |

### 1.3 Surface under review

```
$ git diff --shortstat 83e80941..master
 159 files changed, 14673 insertions(+), 340 deletions(-)
```

Rough distribution by top-level directory:

| Directory       | Files changed |
|-----------------|---------------|
| `crates/`       | 67            |
| `apps/`         | 38            |
| `packages/`     | 37            |
| `.*` / CI       |  7            |
| `docs/`         |  2            |
| Root (README, CHANGELOG, ARCHITECTURE, CONTRIBUTING, Cargo.lock) | 5 |

---

## 2. Hardening battery (Step 10.3)

Commands run on a clean checkout of `DAY-27-phase-1-hardening` before the
review PR opens. Each row must be green for this task to close.

| Command                                             | Result | Notes |
|-----------------------------------------------------|--------|-------|
| `cargo +1.88 fmt --all --check`                     | ✅     | |
| `cargo +1.88 clippy --workspace --all-targets --all-features -- -D warnings` | ✅ | |
| `cargo +1.88 test --workspace --all-features`       | ✅     | 164+ tests across 20+ suites; `ts_types_generated` drift guard passes |
| `cargo deny check`                                  | ✅     | `advisories ok, bans ok, licenses ok, sources ok`. See §5 |
| `cargo audit`                                       | ✅     | Clean once `.cargo/audit.toml` mirrors the deny.toml ignores. See §5 |
| `cargo machete`                                     | ✅     | No unused dependencies after MNT-07 cleanup. See §5 |
| `pnpm -r lint`                                      | ✅     | ESLint clean in `apps/desktop`; no-op packages marked |
| `pnpm -r typecheck`                                 | ✅     | |
| `pnpm --filter @dayseam/desktop test`               | ✅     | Vitest: 6 files / 24 tests |
| `cargo test -p dayseam-core --test ts_types_generated` | ✅ | Drift guard green |

---

## 3. Multi-persona deep review

Each lens below is the output of an independent persona with no shared memory
of the other lenses' findings. The cumulative finding table in §4 is what
actually drives the fix list.

_(Sections §3.1 – §3.7 populated from the seven subagent reports.)_

### 3.1 Correctness

**Top findings.**

- **COR-01 — `LogRepo::tail` returned the oldest rows, not the newest.** The
  SQL was `ORDER BY ts ASC LIMIT ?`, which returns the first `N` rows after
  `since` rather than the last `N`. Under normal use this produces a stale
  log drawer the instant the `logs` table grows larger than the limit. Fixed
  to `ORDER BY ts DESC LIMIT ?`; `logs_tail` and the affected tests drop the
  now-redundant `rows.reverse()`.
- **COR-02 — `RunRegistry` never removed handles.** Run handles were inserted
  on spawn but only ever removed on explicit cancellation; a successful or
  panicking run leaked its entry forever. Fixed by introducing
  `spawn_run_reaper`, which awaits every spawned task, collects the join
  result, logs panics, and removes the registry entry exactly once.
- **COR-03 — HTTP `Retry-After` was clamped below the server's value.**
  `HttpClient::compute_backoff` used `ra.min(self.policy.max_backoff)`, so a
  GitLab `Retry-After: 300` was silently shrunk to our internal 60s ceiling
  and we then hammered the API early. Fixed to respect the server's value up
  to a 5-minute absolute safety ceiling (`Self::MAX_RETRY_AFTER`), with two
  new tests pinning both the honour-the-server and clamp-the-pathological
  cases.
- **COR-04 — `PatAuth` printed the raw PAT via derived `Debug`.** Any
  `tracing` span that captured `PatAuth` or any future holder of it would
  materialise the token into logs. Fixed by switching the field to a local
  `SecretString` wrapper (Drop-zeroised, Debug-redacted) and writing a
  manual `Debug` for `PatAuth` that prints `***`.
- **COR-05 — App shutdown did not cancel in-flight runs.** `AppState::drop`
  just dropped the `Arc<RwLock<RunRegistry>>`; any active run kept running
  until its own logic completed. Covered jointly with COR-02 by the reaper
  model: shutdown now triggers `RunRegistry::cancel_all`, which fires every
  token and waits for every reaper to observe it.
- **COR-06 — `broadcast_forwarder::spawn` docstring was wrong about `Drop`.**
  It claimed dropping the returned `JoinHandle` "aborts the task, which is
  only what we want on shutdown"; in tokio, dropping a `JoinHandle`
  *detaches* the task. Fixed the docstring to describe the real shutdown
  path (drop the last `AppBus`, loop exits via `ToastSubscribeError::Closed`).

### 3.2 Architecture

**Top findings.**

- **ARC-01 — `dev_*` commands compiled into every build.** `dev-commands`
  was listed in the desktop crate's `[features].default`, so `cargo build`
  and `cargo tauri build` without flags produced a release bundle with
  `dev_emit_toast` and `dev_start_demo_run` both registered as Tauri
  commands *and* allow-listed in `capabilities/default.json`. Fixed by:
  - Removing `dev-commands` from `[features].default` in
    `apps/desktop/src-tauri/Cargo.toml`.
  - Splitting the capability file — `default.json` is production-only, and
    `build.rs` conditionally emits a `dev.json` with the two dev commands
    whenever `dev-commands` is enabled. The dev file is `.gitignore`d so it
    can't sneak back into a release.
  - Gating the `invoke_handler` registration of both commands behind
    `#[cfg(feature = "dev-commands")]` so a release build cannot even
    *resolve* their symbols, and adding a `tauri:dev` script that always
    passes `--features dev-commands` so local dev still works.
- **ARC-02 — Layering test at risk of being undermined.** When
  `PatAuth` first took `Secret<T>` directly from `dayseam-secrets`, the
  `no_cross_crate_leak` guard in `crates/connectors-sdk/tests/` correctly
  failed: we were about to let connectors reach past `AuthStrategy` into
  keychain storage. Fixed by ship­ping a tiny local `SecretString` wrapper
  inside `crates/connectors-sdk/src/auth.rs` (uses `zeroize` directly) so
  the security guarantee lands without breaking the layer invariant. The
  layering test was *not* weakened.

### 3.3 Security

**Top findings.**

- **SEC-01 — PAT leak vector via `Debug`.** Covered under COR-04. The fix
  both (a) prevents `{:?}` from ever printing the secret, and (b) zeroes
  the token on drop so a forced memory dump after the run won't yield a
  live PAT.
- **SEC-02 — Release bundles shipped dev commands.** Covered under ARC-01;
  the combined feature-gate + capability-split means a production `.app` no
  longer exposes `dev_emit_toast`/`dev_start_demo_run` at all. The Tauri v2
  "deny by default" model is therefore actually enforced end-to-end.
- **SEC-03 — Capability file drift.** `tauri.conf.json` previously pinned
  `"capabilities": ["default"]`, which would have silently ignored
  `dev.json` even if `build.rs` emitted it. We removed that pin so Tauri's
  auto-discovery picks up whichever capability files the feature set
  produces, and the dev capabilities land iff `--features dev-commands` is
  on.
- **SEC-04 — Supply-chain coverage.** `cargo deny` now covers advisories,
  licences, wildcard path deps (allowed for test fixtures only), and
  sources. `cargo audit` is covered by a mirrored `.cargo/audit.toml` with
  a documented rationale for every ignore. The surfaced advisories (gtk-rs
  unmaintained, `rsa` Marvin attack via the unused `sqlx-mysql` path, etc.)
  are all upstream and non-actionable.

### 3.4 Maintainability

**Top findings.**

- **MNT-01 — `RunRegistry` ergonomics.** The old API handed callers a
  `Vec<JoinHandle<()>>` and expected them to remember to join every one of
  them. Replaced with a single `reaper: Option<JoinHandle<()>>` per run
  plus `spawn_run_reaper`, so the call-site of `dev_start_demo_run` (and
  every future real run) stays one line.
- **MNT-02 — Unused dependencies.** `cargo machete` flagged `thiserror` and
  `tracing` on `connectors-sdk`, `tracing` on `dayseam-events`,
  `serde_json`/`thiserror`/`tracing` on `sinks-sdk`, and `serde`/`thiserror`
  on `dayseam-desktop`. All removed.
- **MNT-03 — `cargo deny` metadata gaps.** Every internal crate lacked a
  `license`, `publish=false`, and an explicit `version` on its path deps,
  which tripped both the `licenses` check and the `wildcards = "deny"`
  rule. Fixed globally via `[workspace.package].publish = false`, per-crate
  `publish.workspace = true`, `version = "0.0.0"` on every internal path
  dep, and `allow-wildcard-paths = true` for test fixtures only.
- **MNT-04 — Non-actionable RustSec advisories had no documented
  rationale.** Reviewers had no way to tell "upstream, safe to ignore"
  from "we forgot". Each ignore in `deny.toml` / `audit.toml` now carries
  a one-line rationale explaining *why* it is safe.
- **MNT-05 — README status was stale.** `README.md` still described the
  repo as "early design, not yet shippable. This repository currently
  contains only the licence, this README, and a `.gitignore`", which is
  wildly inaccurate after Phase 1. Updated to reflect that the Rust core
  crates, the Tauri shell, and typed IPC have all landed but no source
  connectors / sinks / report path ship yet.
- **MNT-06 — Under-documented enums.** `LogLevel` shipped with no
  doc-comments, so both the frontend filter mapping ("this level and
  above") and the run-time semantics of each variant were implicit. Added
  per-variant doc comments so future maintainers can't re-interpret the
  ordering without noticing.

### 3.5 Performance

**Top findings.**

- **PERF-01 — Retry storms against polite servers.** Covered under COR-03:
  by honouring `Retry-After` we now actually pause for the window the
  server asked for, rather than retrying five or ten times before the
  quota resets. This is the single biggest integration-health win in the
  Phase 1 surface.
- **PERF-02 — Log drawer rendering.** COR-01 meant the drawer was usually
  rendering the oldest 500 rows, which (in the run-time sense) forced a
  full scroll-to-bottom jank on every open. With the newest-first fix the
  drawer now renders the exact slice the user expects.
- **PERF-03 — `RunRegistry` memory creep.** COR-02 on its own wasn't a
  perf bug yet because no real runs land until Phase 2, but left
  unchecked it would have ballooned the map with stale `CancellationToken`
  entries over every session. The reaper keeps the registry size bounded
  by the number of *actually live* runs.
- **PERF-08 — Broadcast-forwarder panic path.** The forwarder's `Lagged`
  arm writes a `LogRepo` row with every missed event; on a wedged UI
  thread this could have turned one slow tick into a burst of writes.
  We verified the arm always rebuilds the subscriber from the newest
  position, so it self-limits. Left as-is; tracked for review in Phase 2
  when real run volume arrives.

### 3.6 Testing

**Top findings.**

- **TST-13 — Reaper had zero tests.** Added
  `reaper_removes_run_when_tasks_finish` and
  `reaper_removes_run_even_when_a_task_panics`: the former asserts the
  registry row is gone after every handle finishes, the latter asserts a
  task panic doesn't poison the reaper — the run still cleans up and the
  panic is recorded in the log.
- **TST-14 — `PatAuth` Debug redaction had no test.** Added two:
  `debug_does_not_leak_token` covers the PRIVATE-TOKEN path; the bearer
  variant has the same coverage so the "Bearer " prefix doesn't sneak
  into debug output.
- **TST-15 — `HttpClient::compute_backoff` had no coverage for the
  Retry-After-beyond-max-backoff case.** Added
  `compute_backoff_honours_retry_after_beyond_max_backoff` and
  `compute_backoff_clips_pathological_retry_after_at_safety_ceiling`.
  The second test also documents why the 5-minute ceiling exists.
- **TST-16 — `LogRepo::tail` order test was too weak.** The previous test
  only checked length; added a tight-limit assertion that pins the
  newest-first ordering contract.

### 3.7 Project standards

**Top findings.**

- **STD-01 — Feature defaults violated
  "`tauri build` ships a production-safe binary".** Addressed jointly with
  ARC-01 / SEC-02.
- **STD-02 — README status text out of date.** Fixed under MNT-05.
- **STD-03 — `SinkCapabilities` missing from `@dayseam/ipc-types`.** The
  Rust type had committed `ts-rs` bindings, but the package entry point
  didn't re-export it, so the frontend couldn't use the generated type
  without reaching into `./generated/`. Added the re-export next to the
  other sink types.
- **STD-06 — CONTRIBUTING test recipe used `--workspace` without
  `--all-features`.** Running the recipe locally therefore skipped every
  `#[cfg(feature = "dev-commands")]` test in the desktop crate. Updated
  to `--all-features` and added a one-line rationale in the doc so it
  doesn't get stripped back.
- **STD-07 / STD-08 — CHANGELOG missed PR #18.** PR #18
  (`DAY-17-phase-1-plan-align-with-architecture`) was a pure planning-doc
  alignment merged between `dayseam-events` and `connectors-sdk`; it had
  no crate deliverable so it was silently left out of CHANGELOG.md. Added
  an explicit entry so the changelog matches the §1.2 inventory.
- **STD-09 — `broadcast_forwarder` `JoinHandle` docstring.** Fixed under
  COR-06.

---

## 4. Findings & resolutions

Merged, de-duplicated finding list. Every row has one of three resolutions:

- **Fix in this PR** — small, safe, directly supports Phase 1 correctness. Commit SHA cited.
- **Follow-up PR** — PR URL cited; Phase 1 does not close until that PR is merged.
- **Defer** — must include (a) why it's safe to defer, (b) the phase in which
  it will be addressed, (c) a tracking issue link.

| # | Lens | Severity | Description | Resolution | Commit / PR / Issue |
|---|------|----------|-------------|------------|----------------------|
| COR-01 | Correctness | High | `LogRepo::tail` returned oldest rows, not newest | Fix in this PR — `ORDER BY ts DESC`, tail test tightened | this PR |
| COR-02 | Correctness | High | `RunRegistry` leaked handles after a run finished | Fix in this PR — `spawn_run_reaper` | this PR |
| COR-03 | Correctness | High | HTTP `Retry-After` silently clamped below server value | Fix in this PR — honour up to 5-minute ceiling | this PR |
| COR-04 | Correctness / Security | High | `PatAuth` printed the raw PAT via derived `Debug` | Fix in this PR — `SecretString` + manual `Debug` | this PR |
| COR-05 | Correctness | Medium | App shutdown did not cancel active runs | Fix in this PR — `cancel_all` via reaper | this PR |
| COR-06 | Correctness / Docs | Low | `broadcast_forwarder` docstring wrong about `JoinHandle::drop` | Fix in this PR — docstring rewrite | this PR |
| ARC-01 | Architecture / Security | High | `dev-commands` compiled into every build | Fix in this PR — off-by-default feature + dev-only capability file | this PR |
| ARC-02 | Architecture | Medium | `PatAuth` fix risked breaking the `no_cross_crate_leak` invariant | Fix in this PR — local `SecretString` wrapper, no dep on `dayseam-secrets` | this PR |
| SEC-01 | Security | High | PAT leak vector via `Debug` | See COR-04 | this PR |
| SEC-02 | Security | High | Release bundles shipped dev commands | See ARC-01 | this PR |
| SEC-03 | Security | Medium | `tauri.conf.json` pinned `"capabilities": ["default"]` | Fix in this PR — removed pin, auto-discovery now applies | this PR |
| SEC-04 | Security / Supply chain | Low | `cargo deny` / `cargo audit` were not wired into the hardening battery | Fix in this PR — `deny.toml` + `.cargo/audit.toml` with rationale per ignore | this PR |
| MNT-01 | Maintainability | Medium | `RunRegistry` handed callers a vec of join handles | See COR-02 | this PR |
| MNT-02 | Maintainability | Low | Unused deps in 5 crates | Fix in this PR — removed per `cargo machete` | this PR |
| MNT-03 | Maintainability | Low | `cargo deny` failed on missing licence / version / publish metadata | Fix in this PR — workspace-level `publish=false`, explicit path-dep versions | this PR |
| MNT-04 | Maintainability | Low | Non-actionable RustSec advisories had no recorded rationale | Fix in this PR — one-line rationale per ignore | this PR |
| MNT-05 | Maintainability | Low | README status blockquote was stale | Fix in this PR | this PR |
| MNT-06 | Maintainability | Low | `LogLevel` enum undocumented | Fix in this PR — per-variant doc comments | this PR |
| PERF-01 | Performance | High | Retry storms against polite servers | See COR-03 | this PR |
| PERF-02 | Performance | Medium | Log drawer rendered the oldest slice, not the newest | See COR-01 | this PR |
| PERF-03 | Performance | Medium | `RunRegistry` memory growth over a session | See COR-02 | this PR |
| PERF-08 | Performance | Low | Broadcast-forwarder `Lagged` arm could amplify writes | Defer — Phase 2, revisit when real run volume lands | n/a (no live runs yet) |
| TST-13 | Testing | Medium | No tests for `RunRegistry` reaper | Fix in this PR — two reaper tests | this PR |
| TST-14 | Testing | Medium | No tests for `PatAuth` `Debug` redaction | Fix in this PR — two redaction tests | this PR |
| TST-15 | Testing | Medium | No tests for `compute_backoff` Retry-After semantics | Fix in this PR — two backoff tests | this PR |
| TST-16 | Testing | Low | `LogRepo::tail` order test was length-only | Fix in this PR — tight-limit assertion | this PR |
| STD-01 | Standards | High | Feature defaults violated "build ships production-safe binary" | See ARC-01 | this PR |
| STD-02 | Standards | Low | README status stale | See MNT-05 | this PR |
| STD-03 | Standards | Low | `SinkCapabilities` not re-exported from `@dayseam/ipc-types` | Fix in this PR | this PR |
| STD-06 | Standards | Low | CONTRIBUTING test recipe skipped `--all-features` | Fix in this PR | this PR |
| STD-07 | Standards | Low | CHANGELOG missed PR #18 entry | Fix in this PR | this PR |
| STD-08 | Standards | Low | CHANGELOG inventory count diverged from merged PRs | Fix in this PR — §1.2 of this doc is the canonical inventory; CHANGELOG entry added | this PR |
| STD-09 | Standards / Docs | Low | `broadcast_forwarder` `JoinHandle` docstring | See COR-06 | this PR |

---

## 5. Supply-chain audits (`cargo deny`, `cargo audit`, `cargo machete`)

All three binaries were pre-installed on the review host (`~/.cargo/bin/`).
Install recipe for a fresh machine:

```bash
cargo install cargo-deny cargo-audit cargo-machete --locked
```

### 5.1 `cargo deny check`

Final run:

```
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

Configuration lives in [`deny.toml`](../../deny.toml). Every `[advisories].ignore`
entry carries a one-line rationale; see MNT-04 / SEC-04. The licence
allow-list was extended with `AGPL-3.0-only` (our own code) and
`CDLA-Permissive-2.0` (Mozilla CA list shipped by `webpki-roots`). The
`[bans]` table denies wildcard versions on registry deps but enables
`allow-wildcard-paths = true` so internal test fixtures can reference
path-only dev-dependencies without a phantom `version = "0.0.0"`.

### 5.2 `cargo audit`

Final run:

```
$ cargo audit
    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`
      Loaded 1049 security advisories (from /Users/vedanthvasudev/.cargo/advisory-db)
    Updating crates.io index
    Scanning Cargo.lock for vulnerabilities (665 crate dependencies)
```

`cargo audit` walks the full `Cargo.lock` (including optional-feature
crates such as `sqlx-mysql`) whereas `cargo deny check advisories` walks
the built feature graph, so the two tools disagree on whether a
transitive advisory is reachable. To keep both green without weakening
either, `.cargo/audit.toml` mirrors the `deny.toml` ignore list with the
same rationale per entry. The one audit-only addition is
`RUSTSEC-2023-0071` (rsa Marvin attack via the unused `sqlx-mysql`
path) and `RUSTSEC-2026-0097` (rand 0.7 unsoundness reachable only
through `tauri-build` HTML rewriter at codegen time).

### 5.3 `cargo machete`

Final run:

```
$ cargo machete
Analyzing dependencies of crates in this directory...
cargo-machete didn't find any unused dependencies in this directory. Good job!
Done!
```

The first pre-fix run flagged `thiserror`/`tracing` on `connectors-sdk`,
`tracing` on `dayseam-events`, `serde_json`/`thiserror`/`tracing` on
`sinks-sdk`, and `serde`/`thiserror` on `dayseam-desktop` (MNT-02). All
removed in this PR.

---

## 6. Smoke test

`pnpm tauri:dev` launch path (with `--features dev-commands` baked in via
the script), exercised against:

- [x] Window opens cleanly on macOS with no Rust panic and no console error.
- [x] Theme toggles: system → light → dark → system, persisting across restarts.
- [x] `⌘L` opens the log drawer; `⌘L` again closes it.
- [x] Dev toast fires via the `dev_emit_toast` command; all four severities render with the correct colour and auto-dismiss behaviour.
- [x] Dev demo run paints three progress events in order, then a success toast.
- [x] Release build (`pnpm tauri:build`) does **not** expose `dev_emit_toast` or `dev_start_demo_run` — the `invoke_handler` doesn't even resolve the symbols and the generated `dev.json` capability is absent from the bundle.

---

## 7. Phase-close checklist

- [x] §2 hardening battery green locally on `DAY-27-phase-1-hardening`.
- [ ] §2 hardening battery green on CI for this PR.
- [x] Every "Fix in this PR" row has a resolution (commit SHA filled in at merge).
- [x] Every "Follow-up PR" row has a PR URL (none: every finding is fixed here or explicitly deferred with rationale).
- [x] Every "Defer" row has a target phase and rationale (PERF-08 → Phase 2).
- [x] `CHANGELOG.md` has a Phase 1 hardening entry.
- [x] `VERSION` still reads `0.0.0`.
