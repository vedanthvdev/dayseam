import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { THEME_STORAGE_KEY, type Theme } from "../theme";
import {
  registerOnboardingComplete,
  resetTauriMocks,
} from "./tauri-mock";

// Inline snapshots of the rendered DOM per theme. They exist to make
// layout drift a reviewed event rather than an accidental one. If a
// diff shows up, regenerate with `pnpm -F @dayseam/desktop test -u`,
// eyeball the change in the PR, and only then commit.

async function renderWithTheme(theme: Theme): Promise<HTMLElement> {
  localStorage.setItem(THEME_STORAGE_KEY, theme);
  const { container } = render(<App />);
  // Wait for the main layout to settle after the setup checklist
  // gate resolves, so the snapshot captures the stable "fully
  // onboarded" frame rather than an intermediate loading state.
  await waitFor(() =>
    expect(
      screen.getByRole("region", { name: /connected sources/i }),
    ).toBeInTheDocument(),
  );
  // DAY-170: the merged `SourcesSidebar` auto-selects every
  // configured source on the frame after `useSources` resolves.
  // On fast local runs that happens within the same tick as the
  // region appearing, but CI has been observed to capture the
  // transient "region present, nothing selected, Generate
  // disabled" frame. Wait for Generate to be enabled so the
  // snapshot always reflects the settled state.
  await waitFor(() =>
    expect(
      screen.getByTestId("action-row-generate"),
    ).not.toBeDisabled(),
  );
  // `useLocalRepos` resolves on a separate microtask chain from
  // `useSources`, so the chip can still show the `· …` placeholder
  // even after Generate is enabled. The snapshot is meant to
  // capture the fully-loaded state, so wait for the repo count to
  // materialise before taking it. `findBy*` polls until the
  // placeholder is replaced with the "N repo(s)" label.
  await screen.findByText(/· \d+ repos?/);
  return container;
}

// Attributes that can drift run-to-run (react-generated ids, test
// ordering, today's date on the merged report-row date picker) are stripped
// so the snapshot stays meaningful and doesn't flake across
// midnight. The date input's initial `value` is derived from the
// user's local calendar day via `localTodayIso()`, so without
// normalising it the snapshot needs accepting once per day.
function sanitize(html: string): string {
  return html
    .replace(/\s+data-reactroot=""/g, "")
    .replace(/id=":[^"]+"/g, 'id="<stable>"')
    .replace(
      /(data-testid="action-row-date"[^>]*value=")\d{4}-\d{2}-\d{2}(")/,
      '$1<today>$2',
    );
}

describe("App visual shape", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
    resetTauriMocks();
    // The snapshot is meant to capture the steady-state main layout;
    // registering the fully-onboarded fixture keeps the gate off.
    registerOnboardingComplete();
  });

  afterEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
    resetTauriMocks();
  });

  it("renders the light-theme DOM shape", async () => {
    const container = await renderWithTheme("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(sanitize(container.innerHTML)).toMatchSnapshot();
  });

  it("renders the dark-theme DOM shape", async () => {
    const container = await renderWithTheme("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(sanitize(container.innerHTML)).toMatchSnapshot();
  });
});
