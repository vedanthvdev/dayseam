import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Dialog, DialogButton } from "../components/Dialog";

describe("Dialog primitive", () => {
  it("mounts nothing when open is false", () => {
    render(
      <Dialog open={false} onClose={() => {}} title="Title" testId="d">
        body
      </Dialog>,
    );
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("renders the title, description, and body when open", () => {
    render(
      <Dialog
        open
        onClose={() => {}}
        title="Add source"
        description="Provide a scan root."
        testId="d"
      >
        <p>inner</p>
      </Dialog>,
    );
    expect(screen.getByRole("dialog", { name: /add source/i })).toBeInTheDocument();
    expect(screen.getByText(/provide a scan root/i)).toBeInTheDocument();
    expect(screen.getByText(/inner/i)).toBeInTheDocument();
  });

  it("closes on Escape", () => {
    const onClose = vi.fn();
    render(
      <Dialog open onClose={onClose} title="T" testId="d">
        body
      </Dialog>,
    );
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on backdrop click, not on content click", () => {
    const onClose = vi.fn();
    render(
      <Dialog open onClose={onClose} title="T" testId="d">
        <button type="button">inside</button>
      </Dialog>,
    );
    // Click content first — must not close.
    fireEvent.mouseDown(screen.getByRole("button", { name: /inside/i }));
    expect(onClose).not.toHaveBeenCalled();

    // Click backdrop — must close.
    fireEvent.mouseDown(screen.getByTestId("d-backdrop"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("restores focus to the opener when it unmounts", () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button
            type="button"
            data-testid="opener"
            onClick={() => setOpen(true)}
          >
            open
          </button>
          <Dialog open={open} onClose={() => setOpen(false)} title="T" testId="d">
            body
          </Dialog>
        </>
      );
    }
    render(<Harness />);
    const opener = screen.getByTestId("opener");
    opener.focus();
    fireEvent.click(opener);
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(document.activeElement).toBe(opener);
  });
});

describe("DialogButton", () => {
  it("forwards clicks and respects the disabled prop", () => {
    const onClick = vi.fn();
    const { rerender } = render(
      <DialogButton kind="primary" onClick={onClick}>
        Go
      </DialogButton>,
    );
    fireEvent.click(screen.getByRole("button", { name: /go/i }));
    expect(onClick).toHaveBeenCalledTimes(1);

    rerender(
      <DialogButton kind="primary" onClick={onClick} disabled>
        Go
      </DialogButton>,
    );
    fireEvent.click(screen.getByRole("button", { name: /go/i }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  // DAY-128 #1: the old DialogButton relied on `py-1.5 leading-5`,
  // which left the glyph baseline visibly low inside the button
  // chrome — readers perceived the primary button (e.g. "Done" on
  // the Identities dialog, "Add source" on the Atlassian add dialog)
  // as sitting lower on the page than it should. The fix centres
  // the glyph geometrically via flex + a fixed height, so this test
  // pins the centering classes so a future refactor doesn't
  // silently regress to the old line-height-driven layout.
  it("centers the glyph geometrically with flex + a fixed height", () => {
    render(<DialogButton kind="primary">Done</DialogButton>);
    const button = screen.getByRole("button", { name: /done/i });
    expect(button.className).toContain("inline-flex");
    expect(button.className).toContain("items-center");
    expect(button.className).toContain("justify-center");
    expect(button.className).toContain("h-8");
    expect(button.className).toContain("leading-none");
    // `py-1.5` is what put the glyph optically low; the fix replaces
    // it with the fixed-height/flex-center combo above. A future
    // change that re-adds `py-1.5` alongside the flex classes should
    // fail this assertion so we stay honest about what produces the
    // right visual result.
    expect(button.className).not.toMatch(/\bpy-1\.5\b/);
  });
});
