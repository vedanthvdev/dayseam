import { render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { THEME_STORAGE_KEY, type Theme } from "../theme";

// Inline snapshots of the rendered DOM per theme. They exist to make
// layout drift a reviewed event rather than an accidental one. If a
// diff shows up, regenerate with `pnpm -F @dayseam/desktop test -u`,
// eyeball the change in the PR, and only then commit.

function renderWithTheme(theme: Theme): HTMLElement {
  localStorage.setItem(THEME_STORAGE_KEY, theme);
  const { container } = render(<App />);
  return container;
}

// Attributes that can drift run-to-run (react-generated ids, test
// ordering) are stripped so the snapshot stays meaningful.
function sanitize(html: string): string {
  return html
    .replace(/\s+data-reactroot=""/g, "")
    .replace(/id=":[^"]+"/g, 'id="<stable>"');
}

describe("App visual shape", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
  });

  afterEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
  });

  it("renders the light-theme DOM shape", () => {
    const container = renderWithTheme("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(sanitize(container.innerHTML)).toMatchSnapshot();
  });

  it("renders the dark-theme DOM shape", () => {
    const container = renderWithTheme("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(sanitize(container.innerHTML)).toMatchSnapshot();
  });
});
