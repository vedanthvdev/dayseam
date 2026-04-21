# Spike: Atlassian (Jira + Confluence) connector data shape

**Branch:** `spike/jira-connector-data-shape`
**Date:** 2026-04-20
**Question:** Before drafting the v0.2 phase plan for Atlassian
support, what does real Jira + Confluence Cloud data actually look
like for an EOD report, do the two products share enough auth and
identity plumbing to be planned together, and how does all of it map
onto Dayseam's existing `SourceConnector` + `ActivityEvent` +
`SourceIdentity` contract?

**Method:** Hit a real Atlassian Cloud instance
(`modulrfinance.atlassian.net`, ~2 years of history, active Jira
projects `CAR` + `KTON` + `SUP`, active Confluence spaces `ST` +
`QT` + `FET`) through the Rovo MCP —
`searchJiraIssuesUsingJql`, `getJiraIssue` with
`expand=changelog,renderedFields`, `searchConfluenceUsingCql`,
`getConfluenceSpaces`, `atlassianUserInfo`,
`getAccessibleAtlassianResources`. Every field below is an
observed field, not a docs-inferred field.

This document is the **spike output**, not a plan. It answers "what
should `connector-jira` and `connector-confluence` look like, and
should they be one crate or two?" so the plan doc (to follow) can
be concrete and right-sized.

## TL;DR

Jira and Confluence on Atlassian Cloud share one `accountId`, one
email+API-token credential, one cloudId, and one hostname — just
different URL prefixes (`/rest/api/3/` vs `/wiki/api/v2/`) and
different CQL/JQL query languages. **Plan them together, ship them
as two sibling connector crates (`connector-jira`,
`connector-confluence`) backed by a shared
`connector-atlassian-common` module.** Combined scope is ~10 working
days (Jira 7 + Confluence 4 − 1 saved by the shared module), versus
~12 days if done back-to-back as separate phases.

---

## 1. What a user actually does on Jira in a day

Empirically, on `2026-04-20` for one account:

| What happened | How it shows up in the API |
|---|---|
| 6 automated status transitions on `CAR-5117` between 08:48:09 and 08:48:19 UTC (`Work In Progress → Awaiting Review → In Review → Release Testing → Awaiting Deployment → Regression Testing → Production Verification`) | Six separate `changelog.histories[].items[]` entries, `field: "status"`, all by the same `author` within a 10-second window |
| Left a comment on `KTON-4550` mentioning a colleague, asking for replication-steps update | One `comment.comments[]` entry, `author.accountId = <self>`, `body` is ADF (rich JSON doc) |
| Received a reply from the colleague on `KTON-4550` | Another `comment` entry with a different `author.accountId` — **not the current user's activity**, should not emit a self-event |
| (Earlier in the week) Closed `CAR-4965`, `CAR-5163`, `SUP-3129` | Final status transition + resolution set — same changelog shape |
| Many `RemoteWorkItemLink` history entries: "This work item links to 'Merge request - CAR-5117: …' (Web Link)" | A changelog item with `field: "RemoteWorkItemLink"`, `toString` carrying the GitLab MR title. These are **cross-source links** Dayseam already tracks via `connector-gitlab` |

**What did not show up:**
- Worklog entries (`worklogAuthor = currentUser()` returned zero
  issues). This org doesn't use Jira time-tracking; worklog support
  should be an **optional** connector capability, not v0.1 scope.
- Issue-create events for the current user (user is mostly an
  assignee, not a reporter, in this project)

## 2. Mapping onto Dayseam's existing contracts

### 2.1 `ActivityKind` additions

The existing enum in [`crates/dayseam-core/src/lib.rs`](../../crates/dayseam-core/src/lib.rs)
already has `CommitAuthored`, `MrOpened`, `MrMerged`, `MrReviewComment`.
Jira needs five new variants, minimally:

| Variant | Source event | Rollup behaviour |
|---|---|---|
| `JiraIssueTransitioned` | One `changelog.histories[].items[]` where `field == "status"` and `author == self` | **Collapse within a window.** Six transitions in 10 s on one issue = one bullet (`WIP → Production Verification`), same rollup mechanism as `annotate_rolled_into_mr` |
| `JiraIssueCommented` | One `comment.comments[]` where `author.accountId == self` | No rollup; each comment is its own bullet |
| `JiraIssueAssigned` | `changelog` item `field == "assignee"`, `toString == self.displayName` | Single bullet "Assigned to you: SUM-123" |
| `JiraIssueCreated` | Issue where `reporter.accountId == self` and `created` is inside the window | Single bullet per issue |
| `JiraIssueResolved` | `status` transition to any status in `statusCategory.key == "done"`, `author == self` | Rollup-aware — the final transition of a rapid cascade should be labelled "Closed CAR-4965" not "6th transition" |

This can start with three (`Transitioned`, `Commented`, `Assigned`)
and defer the other two to v0.2.1.

### 2.2 `Actor` shape

Jira's `author` object carries `accountId` (opaque, stable, canonical)
+ `displayName` + `emailAddress` (often hidden on Cloud Managed for
privacy). Maps 1:1 onto:

```
Actor {
    display_name: "Vedanth VasuDev",
    email: Some("vedanth.vasudev@modulrfinance.com") | None,
    external_id: Some("61128bbf4e8d8d0069e48e16"),  // accountId
}
```

### 2.3 `SourceIdentity` — the self-filter

By analogy with `GitLabUserId`, we need:

- **`JiraAccountId(String)`** — canonical self-match, populated from
  `/rest/api/3/myself` at connect time (auto-seed, exactly like
  DAY-71's `ensure_gitlab_self_identity`).
- **`JiraEmail(String)`** — optional fallback for instances that let
  users opt into email visibility; only matches if the self's
  `accountId` filter produced nothing.

The walker must filter every `history` / `comment` by
`author.accountId == <JiraAccountId value>` before emitting an
event — this is the exact shape DAY-71 fixed for GitLab
(`identity.malformed_user_id` log shows up here too).

### 2.4 `EntityRef` shape

Per CONS-addendum-04 (local-git repo.label parity), the connector
emits consistent entities:

| Kind | external_id | label |
|---|---|---|
| `project` | `"CAR"` (project key, stable across name changes) | `Some("Carbon Team")` |
| `issue` | `"CAR-5117"` (issue key) | `Some("Run services easy mode")` |
| `target` (optional) | parent-issue-key if the issue is a subtask | `Some(parent.summary)` |

`repo_path_from_event` in
[`dayseam-report/src/rollup.rs`](../../crates/dayseam-report/src/rollup.rs)
already groups by the `repo` entity; for Jira the analogue is
**`project`**. The rollup layer should be generalised
(`group_key_from_event`) to fall back to `project` when `repo`
isn't present. (This is a cross-cutting refactor — probably the
first task of the Jira phase plan.)

### 2.5 `Link` shape

Three links per event, all stable:

| Label | URL |
|---|---|
| `"CAR-5117"` | `https://modulrfinance.atlassian.net/browse/CAR-5117` (UI link, what the user wants) |
| `"API"` | `https://modulrfinance.atlassian.net/rest/api/3/issue/CAR-5117` (evidence popover) |
| `"Comment #490883"` (only for `JiraIssueCommented`) | `https://modulrfinance.atlassian.net/browse/CAR-5117?focusedCommentId=490883` |

Learned from CORR-02 (Phase 3): the `/-/api/v4/...` vs `/api/v4/...`
mixup was the root cause; here the Jira equivalent is making sure
we use the `/browse/` UI URL for the user-clickable link and the
`/rest/api/3/` path only for evidence. **Three regression tests**
(one per link) minimum.

## 3. Auth contract

### 3.1 Atlassian Cloud API-token flow (target for v0.2)

- **Credential shape:** `email + api_token` (generated at
  `https://id.atlassian.com/manage/api-tokens`).
- **Transport:** HTTP Basic `Authorization: Basic <base64(email:api_token)>`.
- **Base URL:** `https://<workspace>.atlassian.net/rest/api/3`.
- **Maps cleanly onto existing `AuthStrategy` trait:** a new
  `BasicAuth { email, api_token_secret_id }` variant. The shape is
  symmetric to `PatAuth` — one secret, one header. No OAuth
  code-verifier machinery (that's the v0.2 GitLab OAuth track's
  problem).

### 3.2 Server / Data Center PAT (optional second release)

PAT-only, `Authorization: Bearer <token>`. Drop-in once `BasicAuth`
is wired; identical endpoints under `/rest/api/2/...`. **Out of v0.2
scope** unless the user asks — Modulr's own Atlassian is Cloud.

### 3.3 OAuth 2.0 (3LO) — v0.3+

Sample flow is what the Atlassian MCP uses (we saw the
code-verifier failure first-hand today). Do **not** wire this in
v0.2; it couples Jira to the GitLab-OAuth v0.2 track and blocks on
an Atlassian app registration the user doesn't need for self-hosting.

## 4. Rate limit + pagination story

Empirically:

- **Search (`/rest/api/3/search/jql`):** response carries `isLast:
  bool` and an opaque `nextPageToken` (new token-based pagination,
  `startAt` is deprecated as of 2024). Pages cap at 100 issues.
  Connector uses a `while !is_last { fetch(next_page_token) }` loop
  exactly like `connector-gitlab::walk`'s page loop.
- **Per-issue fetch (`/rest/api/3/issue/{key}?expand=changelog`):**
  one call per issue; rate-limit budget is the concern. A heavy day
  (40 touched issues) = 40 calls. Batching via `/rest/api/3/search`
  with `fields=["changelog"]` returns the changelog inline — one
  call for the whole page. Use that path.
- **Rate limits:** Atlassian Cloud publishes `X-RateLimit-*` response
  headers + returns 429 on exhaustion. Our `HttpClient` retry logic
  already handles 429 + `Retry-After` (shipped in Phase 1). No new
  SDK work needed.

## 5. Body normalisation — Atlassian Document Format (ADF)

Every `body` field (comments, descriptions, worklog comments) is
**ADF**, a rich JSON tree of `{type, content, attrs}` nodes. There
is no plain-text fallback. Example from today's
`KTON-4550` comment:

```json
{
  "type": "doc", "version": 1,
  "content": [{
    "type": "paragraph",
    "content": [
      { "type": "mention", "attrs": { "text": "@Saravanan Ramanathan" } },
      { "type": "text", "text": " while reproducing this bug…" }
    ]
  }]
}
```

For the EOD report we need a **depth-first ADF→text walker** that:

1. Traverses `content[]` recursively.
2. Emits `text` node `text`s verbatim, `mention` attrs' `text`
   verbatim (keeping the `@` prefix), joins paragraphs with `\n`.
3. Strips image / media / emoji nodes (not renderable in markdown
   templates).

Helper belongs in `connector-jira/src/adf.rs`; the public API is one
function: `fn adf_to_plain(body: &serde_json::Value) -> String`.
Regression-test with fixtures for each node type. **~80 lines of
Rust, one weekend.**

## 6. Identity auto-seed

Per DAY-71's lesson (the silent `GitLabUserId` missing-filter bug):

- **`sources_add` for a Jira source must call `GET /rest/api/3/myself`
  in the same IPC txn** and insert a `SourceIdentity { kind:
  JiraAccountId, external_actor_id: response.accountId, source_id:
  Some(new_source_id) }`. Same rollback-on-failure shape as
  `ensure_gitlab_self_identity`.
- **`sources_update`** on a Jira source — same call, idempotent
  (the DB uses `INSERT OR IGNORE` on `(source_id, kind)`).
- The JQL `assignee = currentUser()` / `comment ~ currentUser()`
  / `worklogAuthor = currentUser()` primitives let Jira do the
  self-filter server-side too — but we still need the local
  `SourceIdentity` row because the walker reads events where the
  user participated but is *not* the assignee (e.g., a comment on
  someone else's issue). Belt-and-braces: server-side JQL prefilter
  for bandwidth, client-side `accountId` filter for correctness.

## 7. Cross-source dedup + enrichment opportunity

The single biggest emergent insight from the spike:

> `CAR-5117`'s changelog carries ~20 `RemoteWorkItemLink` entries,
> each a link to a GitLab MR whose title embeds the ticket key
> (`CAR-5117: …`). Dayseam's existing cross-source infrastructure
> can fuse these.

**Two enrichment paths, both cheap:**

1. **Ticket-key extraction from existing events.** `connector-gitlab`
   emits `MrOpened { title: "CAR-5117: Rename commands…" }` and
   `connector-local-git` emits `CommitAuthored { title: "CAR-5117:
   Fix DOCKER_HOST nounset" }`. A regex-driven enrichment step in
   `dayseam-report` extracts `/^([A-Z]+-\d+):/` from the titles and
   **attaches a `target` entity** pointing at the Jira issue. No
   Jira API call required.
2. **`RemoteWorkItemLink` reverse lookup.** When Jira emits a
   `JiraIssueTransitioned` event, the connector looks at the
   changelog's `RemoteWorkItemLink` history items to build a
   `commit_shas` / `mr_iids` set, then `annotate_rolled_into_mr`'s
   cousin can annotate *Jira transitions* with their triggering MR:
   > CAR-5117: WIP → Production Verification (via !321)

This is strictly **additive** — no breaking changes to the existing
connectors. It fits as one discrete task in the Jira phase plan.

## 8. Confluence data shape

### 8.1 What a user actually does on Confluence in a day

Observed on `modulrfinance.atlassian.net` for the same account
across April 2026:

| What happened | How it shows up in the API |
|---|---|
| `2026-04-20 16:04 UTC` — edited "Engineering Rota Subscription" page in space `ST` | `content.type = "page"`, `history.latest = true`, `createdBy` = original creator (not necessarily self), version history tracks actual edit authors |
| `2026-04-10 10:43 UTC` — two inline comments on "Authy - Playwright implementation" in space `FET` | `content.type = "comment"`, `extensions.location = "inline"`, `history.createdBy.accountId = <self>` |
| `2026-04-10 09:51 UTC` — authored "EKS-Managed RabbitMQ: Testing Report" in space `ST` | `content.type = "page"`, `createdDate == lastModified` (first version), `createdBy.accountId = <self>` |
| `2026-04-17 13:46 UTC` — collaborative edit on "Rota for Renovate MR's" page created by someone else | CQL `contributor = currentUser()` returns the page; we need the per-version changelog (`/wiki/rest/api/content/{id}/history`) to confirm *which* edit was self's |

CQL query that surfaced everything:

```
contributor = currentUser() AND lastModified >= "2026-04-01" ORDER BY lastModified DESC
```

Returned 6 results across 3 spaces — scale is comparable to Jira
(a few events per active day), **not** a high-volume stream.

### 8.2 Three `ActivityKind` additions

| Variant | Source event | Rollup behaviour |
|---|---|---|
| `ConfluencePageCreated` | CQL result where `content.type == "page"` and `createdBy.accountId == self` and `createdDate` is inside the window | Single bullet per page |
| `ConfluencePageEdited` | Version history entry where `by.accountId == self` and version is not the first | **Collapse within a window.** Multiple saves on the same page in the same minute = one bullet (Confluence auto-saves drafts) |
| `ConfluenceComment` | CQL result where `content.type == "comment"` and `createdBy.accountId == self`. `extensions.location` disambiguates `"inline"` vs `"footer"` | No rollup; each comment is its own bullet. But **co-rollup with sibling comments on the same page**: "3 inline comments on Authy - Playwright implementation" |

Defer: `ConfluencePageCommented`-by-others (not the user's activity),
`ConfluenceAttachmentAdded` (low signal).

### 8.3 `EntityRef` shape for Confluence

Symmetric to Jira:

| Kind | external_id | label |
|---|---|---|
| `space` | `"ST"` (space key) | `Some("Delivery Tribes")` |
| `page` | `"2001142074"` (content id — opaque numeric string, stable across renames) | `Some("Engineering Rota Subscription")` |
| `target` | parent page id if this is a child edit | `Some(parent.title)` |

**Important:** `space` is the rollup/group key — the report bullet
groups events by space the same way Jira groups by project and
GitLab groups by repo. That's why the rollup generalisation
(`group_key_from_event`) mentioned in §2.4 is the critical
cross-cutting refactor — it unlocks both products at once.

### 8.4 `Link` shape for Confluence

| Label | URL |
|---|---|
| `"Engineering Rota Subscription"` (page title) | `https://modulrfinance.atlassian.net/wiki/spaces/ST/pages/2001142074/Engineering+Rota+Subscription` (the `_links.webui` value, which is already URL-encoded) |
| `"API"` | `https://modulrfinance.atlassian.net/wiki/rest/api/content/2001142074` |
| `"Comment #6239617072"` (only for `ConfluenceComment`) | `https://modulrfinance.atlassian.net/wiki/spaces/FET/pages/6222414046/Authy+-+Playwright+implementation?focusedCommentId=6239617072` (from the CQL result's `url` field, relative to `_links.base`) |

Regression test: link URLs must never double-encode the `+` / `%20`
sequences already present in Confluence's `webui` field.

### 8.5 Body normalisation — ADF **or** storage format (XHTML-ish)

Jira comments are always ADF. Confluence bodies can be either,
depending on the endpoint:

- `/wiki/api/v2/pages/{id}` with `body-format=storage` → XHTML-like
  storage format (`<p>…</p><ac:macro …/>`)
- `/wiki/api/v2/pages/{id}` with `body-format=atlas_doc_format` →
  ADF (same as Jira)
- CQL search `excerpt` → already plain-text, good for bullet
  previews but lossy (truncated at ~200 chars)

**Simplifying decision for v0.2:** always request
`body-format=atlas_doc_format` so the ADF walker from §5 is the
only body-normalisation path. Drop storage-format support as a
v0.3 follow-up (or never, if ADF covers every field the report
renders).

### 8.6 Pagination + rate limits

- CQL search returns `_links.next` (a relative URL with an opaque
  cursor) until exhausted. Same loop shape as Jira's
  `nextPageToken`.
- `/wiki/api/v2/spaces` uses the v2 API's cursor pagination
  (`_links.next` + `cursor=eyJ…`). Identical handling.
- Rate limits are per-user, budget is small (~10 pages of activity
  per day worst-case = 1 CQL call + N page-fetch calls for bodies).

### 8.7 Self-identity

Exactly the same as Jira's `JiraAccountId` because Atlassian Cloud
accounts are **unified across products** — the `accountId`
`61128bbf4e8d8d0069e48e16` appears identically in
`atlassianUserInfo`, Jira issue `assignee`, Confluence page
`createdBy`, and Confluence comment `author`. This is the single
biggest reason to plan both connectors together:

> **One `AtlassianAccountId` SourceIdentity row serves both Jira and
> Confluence walkers.** Auto-seed once (from `atlassianUserInfo`),
> reuse everywhere.

## 9. Why one auth, two connectors (not one big crate)

The combined probe established:

| Aspect | Jira | Confluence | Shared? |
|---|---|---|---|
| Host | `modulrfinance.atlassian.net` | `modulrfinance.atlassian.net` | ✅ |
| Auth header | `Basic <base64(email:api_token)>` | `Basic <base64(email:api_token)>` | ✅ |
| accountId | `61128bbf4e8d8d0069e48e16` | `61128bbf4e8d8d0069e48e16` | ✅ |
| Base path | `/rest/api/3/` | `/wiki/api/v2/` + `/wiki/rest/api/` | ❌ |
| Query lang | JQL | CQL | ❌ |
| Event kinds | 3-5 (issue-centric) | 3 (page/comment-centric) | ❌ |
| Body format | ADF | ADF (by endpoint-flag choice) | ✅ |
| Entity graph | project → issue → comment | space → page → comment | analogous |
| Rate budget | per-product | per-product | independent |

**Architecture choice: Option C — separate connectors + shared common module.**

```
crates/connectors/
  connector-atlassian-common/   ← NEW
    src/
      auth.rs          ← BasicAuth header builder, cloudId discovery
      identity.rs      ← atlassianUserInfo → SourceIdentity auto-seed
      adf.rs           ← ADF → plain-text walker (shared by both)
      error.rs         ← 401/403 → DayseamError::Auth mapping
      pagination.rs    ← cursor-pagination loop helper
  connector-jira/                ← NEW, depends on atlassian-common
    src/
      lib.rs           ← impl SourceConnector
      walk.rs          ← JQL search + changelog expansion
      normalise.rs     ← Jira history item → ActivityEvent
  connector-confluence/          ← NEW, depends on atlassian-common
    src/
      lib.rs           ← impl SourceConnector
      walk.rs          ← CQL search + version-history expansion
      normalise.rs     ← Confluence result → ActivityEvent
```

**Rationale:**

1. **Separate SourceConnector impls** — the orchestrator registers
   two distinct connector kinds (`kind() = "jira"` and `"confluence"`),
   which means a user can disable one without the other. Solves the
   "just Jira, no Confluence chatter" UX concern. Maps cleanly onto
   the existing single-kind-per-row Sources table.
2. **Shared crate** — the ADF walker, auth header, identity seed,
   cloudId discovery, and error taxonomy are literally identical.
   Duplicating them across two connectors would violate the
   "cross-source consistency" bar DAY-72 just established.
3. **UI simplification** — the Add-Source dialog can *optionally*
   offer a "Add Jira and Confluence together" shortcut that creates
   two source rows sharing one secret_id, but the connectors
   themselves stay ignorant of this.
4. **Rollup generalisation is needed regardless** — §2.4's move from
   `repo_path_from_event` to `group_key_from_event` is required for
   Jira alone. Confluence rides on top for free.

## 10. Credential sharing in the schema

The existing `sources` table has `secret_id` pointing into the
OS keyring. If the user adds both Jira and Confluence for the same
workspace, **two source rows share one secret_id**:

```
sources:
  id=<uuid-1> kind=jira         name="Jira @ modulrfinance"      secret_id=atl-tok-abc
  id=<uuid-2> kind=confluence   name="Confluence @ modulrfinance" secret_id=atl-tok-abc
```

`secrets_remove(secret_id)` becomes reference-counted, or we add
a `DELETE CASCADE` guard that refuses to drop a secret while any
source still references it. The latter is simpler and matches
what SQLite already does on FK constraints. **One-line migration,
already de-risked** — the `sources` table already has the shape.

## 11. Proposed combined phase decomposition

Not a commitment — a sketch for the plan doc that follows this
spike. Numbers are day-estimates. Ordering matters: tasks 1-3
unlock both connectors; 4-6 ship Jira first (higher signal for a
dev's EOD report); 7-9 ship Confluence; 10-12 close out the phase.

| # | Task | Crate(s) | Estimate | Gating test |
|---|------|----------|----------|-------------|
| 1 | `dayseam-core`: add `AtlassianAccountId` identity kind, seven new `ActivityKind` variants (4 Jira + 3 Confluence), `project` + `space` + `issue` + `page` entity conventions | core | 0.5 d | Invariant test "every ActivityKind has a golden fixture" |
| 2 | `connectors-sdk`: `BasicAuth { email, api_token_secret_id }` auth strategy + wiremocked 401/403 mapping | sdk | 0.25 d | `basic_auth_header_is_base64_email_colon_token`, `401_returns_DayseamError_Auth` |
| 3 | `connector-atlassian-common`: new crate. ADF→text walker, cloudId discovery via `getAccessibleAtlassianResources`, identity seed via `/myself`, cursor-pagination helper | atlassian-common | 1.0 d | ADF fixture suite (5 node types), cloud-discovery wiremock test |
| 4 | `connector-jira`: new crate. `kind()`, `validate_auth()`, `list_identities()` delegating to atlassian-common | jira | 0.25 d | `jira.auth.invalid_token` wiremock |
| 5 | `connector-jira::walk`: JQL search with `expand=changelog`, pagination, transition/comment/assign events | jira | 1.5 d | `walk_day_emits_transition_on_status_change`, `walk_day_collapses_rapid_transitions`, `walk_day_filters_by_account_id` |
| 6 | `dayseam-report`: generalise `repo_path_from_event` → `group_key_from_event` (unblocks both connectors) + ticket-key enrichment (GitLab → Jira `target`) + reverse-lookup MR→transition annotation | report | 1.0 d | No regression in Phase 3 goldens; new `events_group_by_project_or_space`, `commit_titled_CAR_5117_gains_jira_target_entity` |
| 7 | `connector-confluence`: new crate. `kind()`, `validate_auth()`, `list_identities()` delegating to atlassian-common | confluence | 0.25 d | `confluence.auth.invalid_token` wiremock |
| 8 | `connector-confluence::walk`: CQL search + version history, emits page-created/edited/comment events, collapses rapid edits | confluence | 1.25 d | `walk_day_distinguishes_created_vs_edited`, `walk_day_collapses_rapid_saves`, `walk_day_emits_one_event_per_comment` |
| 9 | `dayseam-db`: reference-counted `secret_id` (or FK-guard on delete); integration test for two-sources-one-secret | db | 0.25 d | `deleting_source_preserves_shared_secret_until_last_reference` |
| 10 | `apps/desktop`: `AddAtlassianSourceDialog` (optionally adds both Jira + Confluence in one flow) + `atlassian_validate_credentials` IPC + `SourceErrorCard` copy for `jira.*` + `confluence.*` codes | desktop | 1.5 d | Vitest on the dialog; IPC parity test |
| 11 | Orchestrator wiring + registry entries for both connectors + happy-path E2E covering Jira-only, Confluence-only, and both-at-once | orchestrator + e2e | 0.75 d | `atlassian-happy-path.feature` with three scenarios |
| 12 | Hardening + phase review (same shape as Phase 3 Task 8 + DAY-72 addendum) | — | 1.5 d | Phase-N review doc with addendum-template lenses applied up-front |

**Rough total: ~10 working days**, compared to:
- Jira-alone-first: ~7 days
- Confluence-alone-second (rebuilding common infra): ~5 days
- Combined savings: **~2 days** + lower cross-source-consistency
  risk (one ADF walker, one auth path, one identity seed).

## 12. Risks / unknowns (combined)

| Risk | Mitigation |
|---|---|
| ADF has no public Rust crate for text extraction | ~80 LoC of straightforward recursion; fixtures from both products |
| Confluence storage format (XHTML-ish) might leak into some endpoints we hit | Force `body-format=atlas_doc_format` query param everywhere; defer storage format to v0.3 |
| Token-based pagination shape differs subtly between Jira `search/jql` and Confluence v2 — both opaque but different field names (`nextPageToken` vs `_links.next`) | Abstract in atlassian-common `cursor-pagination` helper; two thin adapters |
| User has Jira access but not Confluence (or vice versa) | `getAccessibleAtlassianResources` returns per-product scope sets; `validate_auth` for each connector filters by its own scope. Missing scope → `DayseamError::Auth` with code `{jira,confluence}.auth.missing_scope` |
| Custom Jira workflows vary wildly by org | Store `from`, `to`, `status_category.key` triad (stable across orgs). Report reasons semantically via category, not display name |
| Same ticket touched by both local-git commit and Jira transition — dedup shape? | **Do not dedup across ActivityKinds.** Only enrich via cross-reference (§7). Same rule applies for Confluence page referenced from a Jira description |
| Comment mentions include PII-ish text (email-in-mention) | ADF walker redacts `@` mentions to display name only; no email lands in the rendered bullet |
| Rate-limit burn on a 40-issue + 10-page day | Single paginated `search` call per product with `expand=changelog` / `body-format=adf`; well inside Atlassian's published 1000 req/min per-user budget |
| Reference-counted secrets change semantics of `secrets_remove` | One-line FK guard on delete; already de-risked in §10 |
| `searchJiraIssuesUsingJql` token-pagination shape (Atlassian has been iterating) | Pin fixtures with real responses; wiremock recordings live in `crates/connectors/connector-jira/tests/fixtures/` |
| Custom Jira workflows vary wildly by org — `CAR` has 8+ statuses, another project may have 3 | Keep event shape *status-transition-agnostic*: store `from`, `to`, `status_category.key` (the stable `new` / `indeterminate` / `done` triad) so the report can reason semantically even when display names differ |

## 13. Recommendation

**Greenlight a combined Jira + Confluence phase (v0.2).** Scope is
understood for both products, auth is literally one credential
serving both, no schema migrations required (the `sources` table
already supports one-secret-many-sources by setting the same
`secret_id` on both rows + a FK guard on delete), cross-source
enrichment between GitLab↔Jira↔Confluence is the long-term
compound payoff. ADF is the only mildly novel sub-task and it's
bounded; both products consume it identically.

Sibling crates (`connector-jira`, `connector-confluence`) backed
by `connector-atlassian-common` keep the code cohesive without
sacrificing the user's ability to enable one product independently
of the other.

Next step: draft `docs/plan/2026-04-2X-v0.2-atlassian.md` mirroring
the phase-3 plan's shape, with the 12 tasks from §11 as the spine.

## 14. Evidence from the spike

### Jira

- `getAccessibleAtlassianResources` → cloudId
  `b5f50ff5-7c51-43af-8bd7-a35cc6801b91`, workspace
  `modulrfinance`, scopes include both `read:jira-work` and the full
  Confluence scope set — **single resource carries both products'
  scopes under one auth**
- `atlassianUserInfo` → account `61128bbf4e8d8d0069e48e16`,
  displayName "Vedanth VasuDev",
  email `vedanth.vasudev@modulrfinance.com`
- `searchJiraIssuesUsingJql` → 4 issues touched in the last 7 days
  (`CAR-5117`, `CAR-4965`, `CAR-5163`, `SUP-3129`); today's report
  would have 1 Jira bullet (`CAR-5117`)
- `getJiraIssue CAR-5117 expand=changelog` → 45 history entries,
  including the rapid 6-transition cascade at 08:48 UTC today +
  20 `RemoteWorkItemLink` entries connecting to GitLab MRs
- `searchJiraIssuesUsingJql comment ~ currentUser()` → 1 issue
  (`KTON-4550`), one comment authored today at 14:16 UTC
- `searchJiraIssuesUsingJql worklogAuthor = currentUser()` → empty
  (org doesn't use worklog)

### Confluence

- `searchConfluenceUsingCql contributor = currentUser() AND lastModified >= "2026-04-01"`
  → 6 results across 3 spaces (`ST`, `QT`, `FET`): 4 page edits + 2
  inline comments, all correctly attributed to the same accountId
  as Jira
- Today's Confluence bullet would be:
  **Delivery Tribes (ST)** — Edited "Engineering Rota Subscription"
- Content types observed: `page` (`createdBy` + versioned), `comment`
  (`extensions.location = inline | footer`)
- Same `accountId` `61128bbf4e8d8d0069e48e16` appears in every
  `history.createdBy` and `comment.author` — confirms unified
  Atlassian identity across products
