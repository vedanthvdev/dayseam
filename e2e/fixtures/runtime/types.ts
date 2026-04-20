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

export type MockState = {
  invocations: Array<{ cmd: string; args: unknown }>;
  captured: {
    saveCalls: Array<{
      draftId: string;
      sinkId: string;
      destinations: string[];
    }>;
  };
  handlers: MockHandlers;
};

declare global {
  interface Window {
    __DAYSEAM_E2E__?: MockState;
  }
}
