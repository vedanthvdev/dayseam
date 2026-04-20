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
});
