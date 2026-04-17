import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import App from "../App";

describe("App", () => {
  it("renders the Dayseam heading", () => {
    render(<App />);
    expect(
      screen.getByRole("heading", { level: 1, name: /dayseam/i }),
    ).toBeInTheDocument();
  });

  it("renders a theme radio group with light, system, and dark", () => {
    render(<App />);
    const group = screen.getByRole("radiogroup", { name: /theme/i });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /light/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /system/i })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /dark/i })).toBeInTheDocument();
  });
});
