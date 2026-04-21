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
  };
  handlers: MockHandlers;
};

declare global {
  interface Window {
    __DAYSEAM_E2E__?: MockState;
  }
}
