import { render, screen, fireEvent } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import App from "../App";
import { THEME_STORAGE_KEY } from "../theme";

describe("App", () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.classList.remove("dark");
    document.documentElement.removeAttribute("data-theme");
  });

  afterEach(() => {
    localStorage.clear();
  });

  it("renders the Dayseam title bar", () => {
    render(<App />);
    expect(
      screen.getByRole("heading", { level: 1, name: /dayseam/i }),
    ).toBeInTheDocument();
  });

  it("renders every wireframe landmark so the window never looks broken", () => {
    render(<App />);
    expect(screen.getByRole("banner")).toBeInTheDocument(); // <header>
    expect(screen.getByRole("region", { name: /report actions/i })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: /connected sources/i })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: /report preview/i })).toBeInTheDocument();
    expect(screen.getByRole("contentinfo")).toBeInTheDocument(); // <footer>
  });

  it("marks every Phase-1 action as disabled with a helpful title", () => {
    render(<App />);
    const generate = screen.getByRole("button", { name: /generate report/i });
    expect(generate).toBeDisabled();
    expect(generate).toHaveAttribute("title", expect.stringMatching(/phase 2/i));
  });

  it("renders a theme radio group with Light / System / Dark", () => {
    render(<App />);
    const group = screen.getByRole("radiogroup", { name: /theme/i });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^light$/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^system$/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^dark$/i })).toBeInTheDocument();
  });

  it("writes data-theme on <html> when the user picks a concrete theme", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("radio", { name: /^dark$/i }));
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");

    fireEvent.click(screen.getByRole("radio", { name: /^light$/i }));
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
  });

  it("marks the selected theme option via aria-checked", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("radio", { name: /^dark$/i }));
    expect(
      screen.getByRole("radio", { name: /^dark$/i }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: /^light$/i }),
    ).toHaveAttribute("aria-checked", "false");
  });

  it("restores the last persisted theme on mount", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    render(<App />);
    expect(
      screen.getByRole("radio", { name: /^dark$/i }),
    ).toHaveAttribute("aria-checked", "true");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });
});
