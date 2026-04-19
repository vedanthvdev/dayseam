import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

if (!window.matchMedia) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

// The Tauri API is only available inside a Tauri webview. In vitest
// we stub both `core` (invoke / Channel) and `event` (listen / emit)
// with an in-process implementation so components that reach into IPC
// don't blow up the render tree, and individual tests can reach back
// in to drive them via the helpers exported from `__tests__/tauri-mock.ts`.
vi.mock("@tauri-apps/api/core", async () => {
  const mod = await import("./tauri-mock");
  return {
    invoke: mod.mockInvoke,
    Channel: mod.MockChannel,
  };
});

vi.mock("@tauri-apps/api/event", async () => {
  const mod = await import("./tauri-mock");
  return {
    listen: mod.mockListen,
  };
});

// `@tauri-apps/plugin-dialog` reaches into the Tauri webview's native
// IPC bridge the moment it's imported, so in vitest we short-circuit it
// to an in-process responder that tests can drive via `queueDialogOpen`.
vi.mock("@tauri-apps/plugin-dialog", async () => {
  const mod = await import("./tauri-mock");
  return {
    open: mod.mockDialogOpen,
  };
});
