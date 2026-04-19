import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import {
  registerInvokeHandler,
  registerOnboardingComplete,
  resetTauriMocks,
} from "./tauri-mock";

describe("App log drawer shortcut", () => {
  beforeEach(() => {
    resetTauriMocks();
    // Main layout is gated on setup completion; onboard the user so
    // the log drawer and its shortcut are actually rendered.
    registerOnboardingComplete();
    registerInvokeHandler("logs_tail", async () => []);
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
  });
  afterEach(() => {
    resetTauriMocks();
    localStorage.clear();
  });

  it("opens the log drawer when ⌘L is pressed and closes it on ⌘L again", async () => {
    render(<App />);
    // Wait for onboarding / report hooks to settle so the surrounding
    // state updates land inside `act(...)`.
    await screen.findByRole("region", { name: /report actions/i });
    expect(screen.queryByRole("dialog", { name: /log drawer/i })).toBeNull();

    fireEvent.keyDown(window, { key: "l", metaKey: true });
    await waitFor(() =>
      expect(
        screen.getByRole("dialog", { name: /log drawer/i }),
      ).toBeInTheDocument(),
    );

    fireEvent.keyDown(window, { key: "l", metaKey: true });
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: /log drawer/i })).toBeNull(),
    );
  });

  it("opens the log drawer when the footer Logs button is clicked", async () => {
    render(<App />);
    await screen.findByRole("region", { name: /report actions/i });
    fireEvent.click(screen.getByRole("button", { name: /^logs$/i }));
    await waitFor(() =>
      expect(
        screen.getByRole("dialog", { name: /log drawer/i }),
      ).toBeInTheDocument(),
    );
  });
});
