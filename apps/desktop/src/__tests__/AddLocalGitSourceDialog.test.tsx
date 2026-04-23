import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Source } from "@dayseam/ipc-types";
import { AddLocalGitSourceDialog } from "../features/sources/AddLocalGitSourceDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
  mockDialogOpen,
  queueDialogOpen,
} from "./tauri-mock";

const SOURCE: Source = {
  id: "src-new",
  kind: "LocalGit",
  label: "Work",
  config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
  secret_ref: null,
  created_at: "2026-04-17T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

describe("AddLocalGitSourceDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => []);
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("keeps the submit button disabled until label + at least one root are present", async () => {
    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );
    // `findBy*` over `getBy*` so the dialog's mount-time async work
    // (label-suggestion fetch) resolves inside React's automatic
    // `act` boundary — otherwise the trailing `setState` leaks an
    // "update not wrapped in act" warning that TST-05 now fails on.
    const submit = await screen.findByRole("button", { name: /add and scan/i });
    expect(submit).toBeDisabled();

    fireEvent.change(screen.getByRole("textbox", { name: /label/i }), {
      target: { value: "Work" },
    });
    expect(submit).toBeDisabled(); // still no roots

    fireEvent.change(
      screen.getByRole("textbox", { name: /scan roots/i }),
      { target: { value: "/Users/me/code\n" } },
    );
    expect(submit).toBeEnabled();
  });

  it("calls `sources_add` with trimmed scan roots and hands the result to `onAdded`", async () => {
    const onAdded = vi.fn();
    registerInvokeHandler("sources_add", async (args) => {
      expect(args.kind).toBe("LocalGit");
      expect(args.label).toBe("Work");
      const config = args.config as {
        LocalGit?: { scan_roots: string[] };
      };
      expect(config.LocalGit?.scan_roots).toEqual([
        "/Users/me/code",
        "/Users/me/work",
      ]);
      return SOURCE;
    });

    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={onAdded} />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: /label/i }), {
      target: { value: "Work" },
    });
    fireEvent.change(
      screen.getByRole("textbox", { name: /scan roots/i }),
      { target: { value: "  /Users/me/code  \n/Users/me/work\n\n" } },
    );
    fireEvent.click(screen.getByRole("button", { name: /add and scan/i }));
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith(SOURCE));
    expect(mockInvoke).toHaveBeenCalledWith(
      "sources_add",
      expect.any(Object),
    );
  });

  it("Browse… appends the picked folder to the scan roots textarea", async () => {
    queueDialogOpen("/Users/me/picked");

    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: /scan roots/i }), {
      target: { value: "/Users/me/existing" },
    });

    fireEvent.click(screen.getByTestId("add-local-git-browse"));

    await waitFor(() => expect(mockDialogOpen).toHaveBeenCalledTimes(1));
    expect(mockDialogOpen).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false }),
    );

    await waitFor(() => {
      const textarea = screen.getByRole("textbox", {
        name: /scan roots/i,
      }) as HTMLTextAreaElement;
      expect(textarea.value).toBe("/Users/me/existing\n/Users/me/picked");
    });
  });

  it("Browse… is a no-op when the user cancels the picker", async () => {
    // Default behaviour of the mock (empty queue) returns null, which
    // is what the real plugin returns on cancel.
    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: /scan roots/i }), {
      target: { value: "/Users/me/existing" },
    });

    fireEvent.click(screen.getByTestId("add-local-git-browse"));

    await waitFor(() => expect(mockDialogOpen).toHaveBeenCalledTimes(1));
    const textarea = screen.getByRole("textbox", {
      name: /scan roots/i,
    }) as HTMLTextAreaElement;
    expect(textarea.value).toBe("/Users/me/existing");
  });

  it("Browse… does not duplicate a folder that is already in the list", async () => {
    queueDialogOpen("/Users/me/existing");

    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={() => {}} />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: /scan roots/i }), {
      target: { value: "/Users/me/existing" },
    });

    fireEvent.click(screen.getByTestId("add-local-git-browse"));

    await waitFor(() => expect(mockDialogOpen).toHaveBeenCalledTimes(1));
    const textarea = screen.getByRole("textbox", {
      name: /scan roots/i,
    }) as HTMLTextAreaElement;
    expect(textarea.value).toBe("/Users/me/existing");
  });

  it("surfaces backend errors inline without closing the dialog", async () => {
    const onAdded = vi.fn();
    registerInvokeHandler("sources_add", async () => {
      throw new Error("permission denied on /root");
    });
    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={onAdded} />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: /label/i }), {
      target: { value: "Work" },
    });
    fireEvent.change(
      screen.getByRole("textbox", { name: /scan roots/i }),
      { target: { value: "/root" } },
    );
    fireEvent.click(screen.getByRole("button", { name: /add and scan/i }));
    await waitFor(() =>
      expect(
        screen.getByText(/permission denied on \/root/i),
      ).toBeInTheDocument(),
    );
    expect(onAdded).not.toHaveBeenCalled();
  });

  // DAY-106 (F-8 / #113). The overlap guard on the Rust side
  // returns `DayseamError::InvalidConfig` which Tauri serialises
  // to `{variant, data: {code, message}}`. Without a bespoke
  // formatter the dialog used to render the whole JSON object,
  // burying the actionable prose message inside escape sequences.
  // The test pins the happy-path that a DayseamError-shaped error
  // from IPC renders as its plain `data.message` so the user sees
  // exactly what the backend wrote ("Scan root X overlaps with
  // source 'Y' …"), not a JSON blob.
  it("renders a DayseamError from IPC as its prose message, not as raw JSON", async () => {
    const onAdded = vi.fn();
    registerInvokeHandler("sources_add", async () => {
      throw {
        variant: "InvalidConfig",
        data: {
          code: "ipc.source.scan_root_overlap",
          message:
            "Scan root \"/Users/me/code\" overlaps with source \"Work\" (scan root \"/Users/me/code/alpha\"). Remove the other source, or narrow this scan root so no discovered repo would be tracked twice.",
        },
      };
    });
    render(
      <AddLocalGitSourceDialog open onClose={() => {}} onAdded={onAdded} />,
    );
    fireEvent.change(screen.getByRole("textbox", { name: /label/i }), {
      target: { value: "Personal" },
    });
    fireEvent.change(
      screen.getByRole("textbox", { name: /scan roots/i }),
      { target: { value: "/Users/me/code" } },
    );
    fireEvent.click(screen.getByRole("button", { name: /add and scan/i }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/overlaps with source "Work"/i);
    expect(alert).toHaveTextContent(/Remove the other source/i);
    // The raw JSON form would include the `variant` key — if the
    // formatter ever regresses to `JSON.stringify(err)` this guard
    // catches it before the user sees a wall of braces.
    expect(alert.textContent ?? "").not.toContain("variant");
    expect(alert.textContent ?? "").not.toContain("InvalidConfig");
    expect(onAdded).not.toHaveBeenCalled();
  });
});
