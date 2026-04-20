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
  },
} as const;

export type Catalogue = typeof CATALOGUE;
