// Single source of truth for every fixture value the E2E suite
// shows the renderer.
//
// Why a separate file (instead of literals inside
// `tauri-mock-init.ts`): the same ids and paths have to line up in
// at least three places — the mock's IPC responses (page context),
// the page-object locators (Node context, e.g. "pick the sink whose
// id is X"), and the step assertions (Node context, e.g. "the save
// payload targeted sink X"). Dropping the values here and threading
// the catalogue into the mock via `addInitScript(fn, CATALOGUE)`
// gives us one place to edit and one place to audit; drift surfaces
// at TypeScript-compile time, not at scenario-run time.
//
// This file is deliberately *just data* so it can be:
//
//   * imported by Node-side step code,
//   * JSON-serialised by Playwright as an `addInitScript` argument,
//
// without ever pulling a runtime dependency across the boundary. If
// a future fixture needs helper logic (e.g. deriving derived ids),
// add a pure function that takes CATALOGUE and returns the derived
// shape — do not attach methods to the catalogue itself.

export const CATALOGUE = {
  // Stable UUIDs per entity, chosen for eyeball-readability
  // (all-1s for the person, all-2s for the source, …). The mock
  // returns these, the assertions compare against them; changing a
  // value here is a scenario-contract change and should be a
  // dedicated commit.
  ids: {
    person: "11111111-1111-1111-1111-111111111111",
    source: "22222222-2222-2222-2222-222222222222",
    identity: "33333333-3333-3333-3333-333333333333",
    sink: "44444444-4444-4444-4444-444444444444",
    run: "99999999-9999-9999-9999-999999999999",
    draft: "88888888-8888-8888-8888-888888888888",
    // DAY-83 — the Atlassian happy-path scenarios seed freshly-
    // created Jira and Confluence source rows through
    // `atlassian_sources_add`. Pinning both ids ahead of time keeps
    // the mock handler deterministic and lets future assertions
    // name the row explicitly ("the `sources_list` IPC includes
    // the Jira row at `…5…`" reads better than an anonymous UUID).
    atlassianJiraSource: "55555555-5555-5555-5555-555555555555",
    atlassianConfluenceSource: "66666666-6666-6666-6666-666666666666",
    // DAY-99 — the GitHub add-source scenarios seed a freshly-
    // created GitHub source row through `github_sources_add`.
    // Pinning the id lets the mock handler render a deterministic
    // UUID and lets future assertions name the row explicitly.
    githubSource: "77777777-7777-7777-7777-777777777777",
  },

  persons: {
    selfDisplayName: "Dayseam E2E",
  },

  sources: {
    label: "work repos",
    scanRoots: ["/tmp/dayseam-e2e-fixture-repo"],
    // The two repos the `local_repos_list` IPC surfaces. A future
    // scenario that asserts per-repo behaviour can add entries here
    // rather than editing the mock.
    repos: [
      { path: "/tmp/dayseam-e2e-fixture-repo/alpha", label: "alpha" },
      { path: "/tmp/dayseam-e2e-fixture-repo/beta", label: "beta" },
    ],
  },

  identities: {
    gitEmail: "e2e@dayseam.app",
  },

  sinks: {
    label: "daily notes",
    destDirs: ["/tmp/dayseam-e2e-sink"],
    // The full path the sink reports having written to. Matches the
    // string the scenario asserts; if the sink shape changes to emit
    // multiple files per save, add them here and the assertion step
    // updates alongside.
    writtenDestinations: ["/tmp/dayseam-e2e-sink/daily-note.md"],
  },

  draft: {
    templateId: "dayseam.dev_eod",
    templateVersion: "2026-04-20-e2e",
    completedBullets: [
      "Wired up the Playwright E2E happy path",
      "Closed Phase 2 deferral cleanup (DAY-57)",
    ],
    // DAY-83 — per-product bullets the mock appends to the draft
    // when a matching Atlassian source is present at report time.
    // The feature file asserts against these exact strings so the
    // human-readable scenario ("the draft contains the Atlassian
    // Jira bullet") stays tied to the string the mock actually
    // serves — drift surfaces as a failing assertion, not as silent
    // pass.
    atlassianJiraBullet: "Moved CAR-5117 to Production Verification",
    atlassianConfluenceBullet:
      "Published runbook on /wiki/spaces/ENG/pages/release-process",
    // DAY-100 — per-product bullet the mock appends to the draft
    // when a GitHub source is present at report time. Same shape as
    // the Atlassian bullets above: pinned in the catalogue so the
    // scenario's "draft contains the GitHub PR bullet" assertion
    // resolves against the exact string the mock served. A
    // regression that drops the bullet surfaces as a failing count;
    // a regression that drifts the copy fails the substring
    // assertion.
    githubPullRequestBullet: "Opened company/foo#42 — Orchestrator-level GitHub PR",
  },

  // DAY-83 — fixture for the Add-Atlassian-source flow. The Atlassian
  // happy-path scenarios drive the real `AddAtlassianSourceDialog`
  // (URL → validate → Add source) so these values have to satisfy
  // the dialog's client-side validation:
  //   * `workspaceUrl` normalises to `https://<host>` via
  //     `normaliseWorkspaceUrl` (bare-slug form accepted so the mock
  //     exercises the common user-typed shape).
  //   * `email` is a real-looking Atlassian account email.
  //   * `apiToken` is a non-empty placeholder (the mock never checks
  //     the token bytes — it mirrors the happy `GET /myself` shape).
  //   * `accountId` / `displayName` / `cloudId` are the triple the
  //     mocked `atlassian_validate_credentials` returns; the dialog
  //     persists them onto the new `SourceIdentity` via
  //     `atlassian_sources_add`.
  atlassian: {
    workspaceSlug: "dayseam-e2e",
    workspaceUrl: "https://dayseam-e2e.atlassian.net",
    email: "e2e@dayseam.app",
    apiToken: "ATATT-e2e-fixture-token",
    accountId: "557058:e2e-account-id",
    displayName: "Dayseam E2E",
    cloudId: "cloud-id-e2e",
    // SecretRef the mock stamps onto newly-created Atlassian source
    // rows. The shared-PAT Journey A writes the same SecretRef onto
    // both rows; separate-PAT Journeys would write distinct values
    // (no DAY-83 scenario exercises that path today).
    sharedSecretRef: {
      keychain_service: "dayseam.atlassian",
      keychain_account: "slot:e2e-shared-pat",
    },
  },

  // DAY-99 — fixture for the Add-GitHub-source flow. The GitHub
  // happy-path scenarios drive the real `AddGithubSourceDialog`
  // (URL → validate → Add source) so these values have to satisfy
  // the dialog's client-side validation:
  //   * `apiBaseUrl` normalises to `https://api.github.com/` via
  //     `normaliseGithubApiBaseUrl` — the default GitHub cloud
  //     shape is pre-filled so the scenario exercises the common
  //     user path without retyping.
  //   * `pat` is a non-empty placeholder (the mock never checks
  //     the token bytes — it mirrors the happy `GET /user` shape).
  //   * `userId` / `login` / `name` are the triple the mocked
  //     `github_validate_credentials` returns; the dialog persists
  //     them onto the new `SourceIdentity` via `github_sources_add`.
  github: {
    apiBaseUrl: "https://api.github.com/",
    pat: "ghp_e2e_fixture_token",
    userId: 424242,
    login: "dayseam-e2e",
    name: "Dayseam E2E",
    label: "api.github.com",
    secretRef: {
      keychain_service: "dayseam.github",
      keychain_account: "source:77777777-7777-7777-7777-777777777777",
    },
  },
} as const;

export type Catalogue = typeof CATALOGUE;
