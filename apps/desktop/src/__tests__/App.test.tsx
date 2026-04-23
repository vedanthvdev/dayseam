import { act, render, screen, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { THEME_STORAGE_KEY } from "../theme";
import {
  registerInvokeHandler,
  registerOnboardingComplete,
  resetTauriMocks,
} from "./tauri-mock";

describe("App", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
    // The setup checklist gate replaces the main layout with the
    // first-run empty state whenever any of the four inputs is
    // missing. Every test in this suite is about the main layout,
    // so we wire up a fully-onboarded fixture by default and let
    // the "first-run" test override one input to flip the gate.
    resetTauriMocks();
    registerOnboardingComplete();
  });

  afterEach(async () => {
    // `App` mounts async hooks (`useSetupChecklist`, `useReport`)
    // that resolve after the synchronous assertions below finish.
    // Flushing pending microtasks inside `act(...)` absorbs the
    // trailing state updates so React's test-mode warnings stay
    // silent — silencing the "update … was not wrapped in act"
    // noise that otherwise fires between tests and drowns real
    // regressions. See TST-05 in docs/review/phase-2-review.md.
    // The deeper multi-tick microtask/macrotask flush lives in
    // `setup.ts`'s global `afterEach`; this hook only handles the
    // local fixture reset.
    await act(async () => {});
    localStorage.clear();
    resetTauriMocks();
  });

  it("renders the Dayseam title bar", async () => {
    render(<App />);
    // Wait for the async onboarding / report fetches to settle
    // before asserting so the surrounding state updates land inside
    // `act`. The heading itself is synchronous; the `findBy*`
    // indirection is what flushes the effects.
    await screen.findByRole("heading", { level: 1, name: /dayseam/i });
    expect(
      screen.getByRole("heading", { level: 1, name: /dayseam/i }),
    ).toBeInTheDocument();
  });

  it("renders every wireframe landmark so the window never looks broken", async () => {
    render(<App />);
    expect(screen.getByRole("banner")).toBeInTheDocument(); // <header>
    // The main layout is gated on the setup checklist, so these
    // landmarks only appear once the four "persons/sources/identities/
    // sinks" fetches have resolved to a complete state.
    await screen.findByRole("region", { name: /report actions/i });
    expect(screen.getByRole("region", { name: /connected sources/i })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: /report preview/i })).toBeInTheDocument();
    expect(screen.getByRole("contentinfo")).toBeInTheDocument(); // <footer>
  });

  it("replaces the main layout with the first-run empty state when no sources are connected", async () => {
    registerInvokeHandler("sources_list", async () => []);
    render(<App />);
    await screen.findByTestId("first-run-empty-state");
    // And the main "Generate report" button is absent — the user
    // has no way to trigger a run until they finish onboarding.
    expect(
      screen.queryByRole("button", { name: /generate report/i }),
    ).toBeNull();
  });

  it("renders a theme radio group with Light / System / Dark", async () => {
    render(<App />);
    await screen.findByRole("radiogroup", { name: /theme/i });
    const group = screen.getByRole("radiogroup", { name: /theme/i });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^light$/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^system$/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^dark$/i })).toBeInTheDocument();
  });

  it("writes data-theme on <html> when the user picks a concrete theme", async () => {
    render(<App />);
    await screen.findByRole("radio", { name: /^dark$/i });
    fireEvent.click(screen.getByRole("radio", { name: /^dark$/i }));
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");

    fireEvent.click(screen.getByRole("radio", { name: /^light$/i }));
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("marks the selected theme option via aria-checked", async () => {
    render(<App />);
    await screen.findByRole("radio", { name: /^dark$/i });
    fireEvent.click(screen.getByRole("radio", { name: /^dark$/i }));
    expect(
      screen.getByRole("radio", { name: /^dark$/i }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: /^light$/i }),
    ).toHaveAttribute("aria-checked", "false");
  });

  it("restores the last persisted theme on mount", async () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    render(<App />);
    await screen.findByRole("radio", { name: /^dark$/i });
    expect(
      screen.getByRole("radio", { name: /^dark$/i }),
    ).toHaveAttribute("aria-checked", "true");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  // DOGFOOD-v0.4-06: sticky footer regression. The bug was that
  // the shell used `min-h-screen` (unbounded) and the scrolling
  // `<section>` in `StreamingPreview` lacked `min-h-0`, so a tall
  // draft made `<body>` the scroll container and pushed
  // `<footer>` below the fold. The fix pins three invariants
  // together — losing any one of them reopens the bug:
  //
  //   1. The shell is `h-dvh` + `overflow-hidden` so the column
  //      height equals the viewport exactly (not "at least").
  //   2. The preview is `flex-1 min-h-0 overflow-y-auto` so the
  //      only scrollable axis lives inside the preview, not on
  //      the body.
  //   3. The footer is a direct flex child of the shell with no
  //      positional overrides — it sits in the bottom strip.
  //
  // This test locks all three as a single guard so a future
  // refactor that swaps `min-h-0` for "looks cleaner without it"
  // (or reverts the shell to `min-h-screen`) red-fails here
  // instead of in user dogfood.
  it("pins the shell + preview + footer layout so the footer stays visible on long reports", async () => {
    render(<App />);
    const preview = await screen.findByRole("region", { name: /report preview/i });
    const footer = screen.getByRole("contentinfo");
    // DAY-103 F-9: anchor the assertion on a stable `data-testid`
    // on the shell root instead of walking up from the preview.
    // The `parentElement` walk would silently start matching the
    // wrong node the moment a future refactor wraps the preview
    // in a scroll-gradient helper / motion div / `<main>` — at
    // which point the test would "pass" against a wrapper that
    // doesn't even have the invariants we care about.
    const shell = screen.getByTestId("app-shell");

    // Invariant 1: shell is bounded to the viewport.
    expect(shell.className).toMatch(/\bh-dvh\b/);
    expect(shell.className).toMatch(/\boverflow-hidden\b/);
    expect(shell.className).toMatch(/\bflex-col\b/);

    // Invariant 2: preview is the scroll container, not the body.
    expect(preview.className).toMatch(/\bflex-1\b/);
    expect(preview.className).toMatch(/\bmin-h-0\b/);
    expect(preview.className).toMatch(/\boverflow-y-auto\b/);

    // Invariant 3: footer is a direct child of the shell, so it
    // cannot be pushed out by a tall preview. We don't
    // over-assert classnames on the footer itself because it's
    // intentionally a normal flex child (no
    // `position: sticky|fixed`); the layout does the work.
    expect(footer.parentElement).toBe(shell);
    // And the preview is a sibling of the footer inside that same
    // shell, not nested inside some other scroll container.
    expect(preview.parentElement).toBe(shell);
  });
});
