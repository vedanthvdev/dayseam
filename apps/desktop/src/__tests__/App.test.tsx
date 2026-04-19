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
});
