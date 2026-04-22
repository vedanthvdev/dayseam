// Shared types for the E2E Tauri mock. Kept in their own file so the
// init script (which runs inside the Chromium page) and the spec body
// (which runs in Node) can both import them without dragging runtime
// code across the serialisation boundary.

export type MockInvokeCtx = {
  emit: (event: string, payload: unknown) => void;
};

export type MockInvokeHandler = (
  args: Record<string, unknown>,
  ctx: MockInvokeCtx,
) => unknown | Promise<unknown>;

export type MockHandlers = Record<string, MockInvokeHandler>;

export type CapturedAtlassianAddCall = {
  workspaceUrl: string;
  email: string;
  // Matches the IPC contract: Journey C mode 1 passes `null` when
  // reusing an existing keychain row. Captured verbatim (not a
  // boolean flag) so the test body can assert exactly what the
  // renderer sent, not a summarised approximation.
  apiToken: string | null;
  accountId: string;
  enableJira: boolean;
  enableConfluence: boolean;
  reuseSecretRef:
    | { keychain_service: string; keychain_account: string }
    | null;
};

// DAY-87: every `atlassian_sources_reconnect` invoke the renderer
// makes lands here so an `@atlassian-reconnect-ipc-contract`
// scenario can assert the exact payload shape the dialog sent
// (source id + fresh token). Symmetrical with `atlassianAddCalls`.
export type CapturedAtlassianReconnectCall = {
  sourceId: string;
  apiToken: string;
};

// DAY-99: every `github_sources_add` invoke the renderer makes
// lands here so a future `@github-add-ipc-contract` scenario can
// assert the exact payload shape the dialog sent (api base url,
// label, PAT, numeric user id, login handle). Kept symmetrical with
// the Atlassian captures above. DAY-101 (CORR-v0.4-01) widened the
// shape with `login` so the capture can assert the dialog threads
// through the identity-seed fields the walker requires.
export type CapturedGithubAddCall = {
  apiBaseUrl: string;
  label: string;
  pat: string;
  userId: number;
  login: string;
};

export type MockState = {
  invocations: Array<{ cmd: string; args: unknown }>;
  captured: {
    saveCalls: Array<{
      draftId: string;
      sinkId: string;
      destinations: string[];
    }>;
    // DAY-83: capture every `atlassian_sources_add` payload so an
    // `@atlassian-add-ipc-contract`-tagged scenario can assert the
    // exact renderer-side IPC shape (journey A / B / C mode 1 /
    // C mode 2). Kept symmetrical with `saveCalls` above.
    atlassianAddCalls: CapturedAtlassianAddCall[];
    // DAY-87: capture every `atlassian_sources_reconnect` payload
    // so the reconnect-flow scenarios can assert the dialog sent
    // the right source id and a non-empty token without a second
    // round-trip through `invocations`.
    atlassianReconnectCalls: CapturedAtlassianReconnectCall[];
    // DAY-99: capture every `github_sources_add` payload so the
    // GitHub add-source scenarios can assert the renderer sent the
    // normalised URL / label / PAT / numeric user id without a
    // second round-trip through `invocations`.
    githubAddCalls: CapturedGithubAddCall[];
  };
  handlers: MockHandlers;
};

declare global {
  interface Window {
    __DAYSEAM_E2E__?: MockState;
  }
}
