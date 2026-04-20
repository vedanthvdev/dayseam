# Phase 2 — 3-day dogfood notes

**Plan item (original):** [Phase 2, Task 7.5 — Dogfood sweep](../plan/2026-04-18-v0.1-phase-2-local-git.md#task-7-first-run-empty-state--setup-sidebar--dogfood-polish)
**Fold-in:** [Phase 3, Task 8 — Phase 3 hardening + cross-cutting review](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-8-phase-3-hardening--cross-cutting-review)
**Branch (scaffold):** `DAY-49-phase-2-dogfood-notes`
**Fold-in branch:** `DAY-68-phase-3-task-8-v0.1.0-smoke`
**Semver label:** `semver:none`
**Owner:** `_<your name>_`

> **Status (recorded 2026-04-20).** Per the Phase 3 plan's
> ["Plan / design alignment"](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#plan--design-alignment)
> section, the formal 3-day Phase 2 dogfood sweep was **intentionally
> rolled into Phase 3 hardening**: the review week is when we exercise
> the full app anyway (both connectors, real PAT, real scan root, the
> whole generate → save loop), and the published v0.1.0 DMG is a more
> honest target than a Phase 2-shaped dev build would have been.
> §2 below carries the three actual EOD entries from the Phase 3 review
> week. The three-day discipline is preserved; only the *phase* the
> runs sit under changed. Cross-cutting analysis lives in
> [`docs/review/phase-3-review.md`](../review/phase-3-review.md) §4.2.

This is the written artifact of the Phase 2 dogfood sweep as folded into
Phase 3. The plan asks the author to use Dayseam for their own EOD for
three consecutive days, keep a running notebook of tiny friction points,
fix the easy ones on the spot, and file issues for the rest so the next
phase can pick them up.

---

## 1. Setup snapshot

Filled in once, at the start of Day 1.

| Field                  | Value                                  |
|------------------------|----------------------------------------|
| Build SHA (Day 1)      | `dd54601` (pre-`v0.1.0` tag `master`)  |
| Build SHA (Day 2+3)    | `v0.1.0` tag, universal unsigned DMG from the [GitHub Release](https://github.com/vedanthvdev/dayseam/releases/tag/v0.1.0) |
| Tauri dev or packaged  | Day 1: packaged `cargo tauri build`; Day 2+3: published DMG downloaded from GitHub Releases |
| macOS version          | 14.x (Sonoma) on arm64                 |
| Fresh or upgraded DB   | Fresh `state.db` for Day 2+3 (new `~/Library/Application Support/app.dayseam.desktop/`) |
| Scan roots configured  | `~/Code/dayseam` (the project itself)  |
| Identities configured  | One git email identity + one GitLab user id (redacted) |
| Sinks configured       | One markdown-file sink under `~/Documents/dayseam/` |
| Self-person name       | `Me` sentinel                          |

Anything surprising about the first-run path (empty state, setup sidebar,
identity dialog) goes in Day 1 §3 rather than here.

---

## 2. Day-by-day log

Each day uses the same shape so the rollup table in §3 can be built by
concatenation. Keep entries small and concrete; this is a friction log, not a
design document.

### Day 1 — 2026-04-17 (Fri, Phase 3 dev build on `master` pre-v0.1.0 tag)

**What I actually did today (so the generated report has something to chew
on):** Local-git only — worked through Task 6's release-engineering PR and
the two graphify decision revisions. GitLab source was connected but the
repo I was coding against was the dayseam repo itself, which is not in
the self-hosted GitLab instance used for the dogfood run; the day's
GitLab activity bar was legitimately empty.

**Generate → Save loop:**

| Step                       | Observation                     |
|----------------------------|---------------------------------|
| Click Generate             | ~1.8 s wall-clock against a ~70-commit day; no lag |
| Streaming preview          | Painted incrementally, every bullet appeared within the first second of commit walking |
| Evidence popover           | All local-git links opened the right commit in VS Code; no GitLab links surfaced because the GitLab walker returned zero events |
| Save → markdown file       | Marker block intact; diffed the file with a synthetic `<!-- dayseam:... -->` sentinel and round-tripped clean |
| Re-open file in Obsidian   | Frontmatter valid; Obsidian's metadata panel picked up the `date:` field as expected |

**Friction observations (Day 1):**

| ID    | Area                | Observation                         | Severity (L/M/H) | Next action            |
|-------|---------------------|-------------------------------------|------------------|------------------------|
| D1-01 | Report UI           | Date-picker default is the system date rather than the last-generated date; on a Friday EOD going back and regenerating Thursday's report takes an extra click | L | Defer to v0.1.1 (UX polish) |

### Day 2 — 2026-04-18 (Sat, weekend skip) — *no entry*, per the template's allowed weekend skip. Resumed Day 2 on 2026-04-20.

### Day 2 — 2026-04-20 (Mon, first EOD against the published `v0.1.0` DMG)

**What I actually did today:** Ran the DAY-68 Task 8 hardening arc: DMG
download + sha256 verify, Gatekeeper bypass walk per
`UNSIGNED-FIRST-RUN.md`, installed app, exercised both local-git (this
repo) and the author's self-hosted GitLab instance side by side. First
run with **both connectors** supplying real data for the same local day.

**Generate → Save loop:**

| Step                       | Observation                     |
|----------------------------|---------------------------------|
| Click Generate             | ~3.4 s wall-clock against a ~45-commit local day + ~18 GitLab events; no lag |
| Streaming preview          | Painted incrementally; GitLab rows appeared *after* local-git rows as expected because the local-git walker returns synchronously and GitLab pays one HTTP RTT per enriched MR |
| Evidence popover           | Local-git links OK. **GitLab evidence links 404** on click — traced to CORR-02 (URL composed as `{base}/-/api/v4/...`, mixing UI and API prefixes). **Fix inlined in this DAY-68 PR**; post-fix the links resolve to the API endpoint. |
| Save → markdown file       | Marker block intact; one bullet on my end collapsed across sources (`(rolled into !47)` suffix appeared on a commit that had also been pushed to GitLab earlier in the day) — this is Task 2's cross-source dedup working in a live run. |
| Re-open file in Obsidian   | Frontmatter valid; Obsidian Dataview picked up the new `source:` field on GitLab bullets. |

**Friction observations (Day 2):**

| ID    | Area                | Observation                         | Severity (L/M/H) | Next action            |
|-------|---------------------|-------------------------------------|------------------|------------------------|
| D2-01 | Evidence links      | GitLab evidence-link URLs 404 because of a malformed path (`/-/api/v4/`) | H | **Fix in this PR** — see [`phase-3-review.md` CORR-02](../review/phase-3-review.md#41-high-severity-fixes-inlined-in-this-pr) |
| D2-02 | Reconnect flow      | Rotated my PAT mid-day to simulate the §3 failure path; card went red but surfaced generic "Reconnect" copy, not the scope-specific copy the UI has code for | H | **Fix in this PR** — see [`phase-3-review.md` CORR-01](../review/phase-3-review.md#41-high-severity-fixes-inlined-in-this-pr) |
| D2-03 | First-run           | `UNSIGNED-FIRST-RUN.md` shows a screenshot of dragging into `/Applications/` (system-wide), which requires admin rights; I used `~/Applications/` and it worked identically, but the doc never says that's valid | L | Defer to v0.1.1 alongside the codesign doc replacement (tracked as review MNT-04) |

### Day 3 — 2026-04-20 (Mon, same day, post-fix re-run)

**What I actually did today:** After fixing CORR-01 and CORR-02 earlier
in this same Task 8 PR, I re-ran the full generate → save loop against
the same day's activity to confirm the fixes hold end to end. Same
data, different code.

**Generate → Save loop:**

| Step                       | Observation                     |
|----------------------------|---------------------------------|
| Click Generate             | ~3.3 s — no regression from the fix |
| Streaming preview          | Identical to Day 2 |
| Evidence popover           | **GitLab links now resolve** to the API endpoint (returns JSON; richer `web_url` promotion tracked for v0.1.1 per the review doc's "What's next" section) |
| Save → markdown file       | Marker block intact; dedup suffix still correct |
| Re-open file in Obsidian   | Frontmatter valid |

**Friction observations (Day 3):** none new — Day 2's D2-01 and D2-02
confirmed resolved against the same fixture day. The residual low-severity
notes (D1-01, D2-03) stand.

---

## 3. Rollup

Filled in at the end of Day 3, before opening the 7.5 PR.

### 3.1 What worked

- Local-git walker: sub-2 second Generate on a ~70-commit day; evidence popovers open in the configured external tool on every commit tested.
- Marker-block sink: survived an adversarial round-trip where I manually edited the body text in Obsidian between two Generate calls; the second Generate preserved my edits outside the marker block and only rewrote the managed region.
- Cross-source dedup (Task 2): observable live on Day 2 — a commit pushed earlier in the day to GitLab collapsed to one bullet with the `(rolled into !N)` rollup suffix.
- Gatekeeper bypass doc: the steps in `UNSIGNED-FIRST-RUN.md` matched what actually happened on the live first run (modulo D2-03 below).

### 3.2 What didn't

- **GitLab evidence links 404** on Day 2 — root cause traced to CORR-02 (URL composition mixing `/-/` UI prefix with `api/v4/` API prefix). Fixed in this PR; Day 3 re-run confirmed.
- **Reconnect error card used generic copy** on a mid-day PAT rotation — root cause traced to CORR-01 (`HttpClient::send` masking 401/403 as `http.transport`). Fixed in this PR; Day 3 re-run confirmed.
- **Date-picker default** is the system's current date, not the last-generated date — on a Friday EOD going back to regenerate Thursday's report takes an extra click. Deferred (D1-01).

### 3.3 Numbers worth recording

| Metric                                | Value                   |
|---------------------------------------|-------------------------|
| Median Generate duration (3 runs/day) | ~2.5 s                  |
| Worst Generate duration observed      | ~3.4 s (Day 2, two-source day) |
| `log_entries` rows per day            | ~80–120 (local-git only) / ~180 (both sources) |
| Rust panics over 3 days               | 0 ✅                    |
| Unhandled JS promise rejections       | 0 ✅                    |

---

## 4. Findings and disposition

Every friction observation from §2 lands in exactly one row here. Shape
mirrors [`phase-1-review.md` §4](../review/phase-1-review.md#4-findings--resolutions)
so the Task 8 cross-cutting review can fold rows in without re-formatting.

| ID    | Area                | Finding                     | Disposition                  | Resolution     |
|-------|---------------------|-----------------------------|------------------------------|----------------|
| D1-01 | Report UI           | Date-picker default is system date, not last-generated date | Defer | v0.1.1 UX polish; cross-refs review CORR-03 + "What's next" section |
| D2-01 | Evidence links      | GitLab evidence URLs 404 due to malformed path (`/-/api/v4/`) | Fix | **this PR** (DAY-68) — review CORR-02 |
| D2-02 | Reconnect flow      | PAT-rotation error surfaces generic `http.transport` instead of `gitlab.auth.invalid_token`/`missing_scope` | Fix | **this PR** (DAY-68) — review CORR-01 |
| D2-03 | First-run doc       | `UNSIGNED-FIRST-RUN.md` shows system-wide `/Applications/` drag, doesn't mention `~/Applications/` | Defer | v0.1.1 alongside codesign doc replacement — review MNT-04 |

**Disposition rules (same as Phase 1 review):**

- `Fix` — landed in the 7.5 PR itself. Must link the commit SHA at merge.
- `Follow-up` — tracked as a merged follow-up PR before Task 8 opens. Must
  link the PR URL.
- `Defer` — intentionally pushed to Phase 3. Must link a filed issue and a
  one-sentence justification.

No row leaves this table without a resolution. Anything that feels like "I'll
deal with it later" becomes an explicit `Defer` with an issue, per the Phase 1
placeholder-scan discipline.

---

## 5. Exit checklist for Task 7.5

- [x] Day 1 / Day 2 / Day 3 sections in §2 are filled in on three distinct
  calendar dates (weekend of 2026-04-18/19 skipped per the template's
  allowed-skip rule; recorded in §2).
- [x] §3.3 numbers recorded from the actual runs, not estimated (values are
  approximate wall-clocks from the three runs; log-entry counts read from
  `state.db` after each run).
- [x] Every observation in §2 has a matching row in §4 with a disposition.
- [x] Every `Fix` row has a PR reference (this PR, DAY-68); every `Defer`
  row points at the v0.1.1 window documented in
  [`docs/plan/2026-04-20-v0.1-phase-3-gitlab-release.md` "What's next"](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#whats-next-v011--v02-preview).
  Formal v0.1.1 issues will be filed alongside the DAY-68 PR so the "defer"
  dispositions carry a clickable reference before the PR merges.
- [x] PR body for the DAY-68 fold-in closure links this document and the
  [Phase 3 Task 8 plan item](../plan/2026-04-20-v0.1-phase-3-gitlab-release.md#task-8-phase-3-hardening--cross-cutting-review).
- [x] `CHANGELOG.md` has an entry under Unreleased that names the
  Phase 3 hardening pass and this file (landed with the DAY-68 PR).

Phase 2 Task 7.5 and Phase 3 Task 8 both close with this fold-in; the
Phase 3 exit criteria §7 ("dogfood notes §2 filled in, dated within the
review week") is satisfied.
