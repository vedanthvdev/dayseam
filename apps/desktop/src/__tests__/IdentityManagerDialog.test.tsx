import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Person, Source, SourceIdentity } from "@dayseam/ipc-types";
import { IdentityManagerDialog } from "../features/identities/IdentityManagerDialog";
import {
  registerInvokeHandler,
  resetTauriMocks,
  mockInvoke,
} from "./tauri-mock";

const SELF: Person = {
  id: "person-1",
  display_name: "Ada",
  is_self: true,
};

const SOURCE: Source = {
  id: "src-1",
  kind: "LocalGit",
  label: "Work",
  config: { LocalGit: { scan_roots: ["/Users/me/code"] } },
  secret_ref: null,
  created_at: "2026-04-17T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

const IDENTITY: SourceIdentity = {
  id: "id-1",
  person_id: "person-1",
  source_id: null,
  kind: "GitEmail",
  external_actor_id: "ada@example.com",
};

describe("IdentityManagerDialog", () => {
  beforeEach(() => {
    resetTauriMocks();
    registerInvokeHandler("sources_list", async () => [SOURCE]);
    registerInvokeHandler("persons_get_self", async () => SELF);
    registerInvokeHandler(
      "identities_list_for",
      async () => [IDENTITY],
    );
  });
  afterEach(() => {
    resetTauriMocks();
  });

  it("loads the self-person and existing identity mappings", async () => {
    render(<IdentityManagerDialog open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/add mapping for ada/i)).toBeInTheDocument(),
    );
    await waitFor(() =>
      expect(screen.getByText("ada@example.com")).toBeInTheDocument(),
    );
  });

  it("upserts a new identity when the form is submitted", async () => {
    registerInvokeHandler("identities_upsert", async (args) => args.identity as SourceIdentity);
    render(<IdentityManagerDialog open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/add mapping for ada/i)).toBeInTheDocument(),
    );
    fireEvent.change(screen.getByRole("textbox", { name: /identity value/i }), {
      target: { value: "ada@work.example" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^add$/i }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "identities_upsert",
        expect.objectContaining({
          identity: expect.objectContaining({
            external_actor_id: "ada@work.example",
            kind: "GitEmail",
            person_id: "person-1",
          }),
        }),
      ),
    );
  });

  it("deletes an identity when its Remove button is clicked", async () => {
    registerInvokeHandler("identities_delete", async () => null);
    render(<IdentityManagerDialog open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText("ada@example.com")).toBeInTheDocument(),
    );
    fireEvent.click(
      screen.getByRole("button", { name: /delete mapping ada@example\.com/i }),
    );
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith(
        "identities_delete",
        expect.objectContaining({ id: "id-1" }),
      ),
    );
  });

  // DAY-170: every identity row carries the coloured brand mark of
  // the connector it maps to, matching the "only colour in the app
  // is a connector brand" convention set on the sources strip. The
  // mapping is: a `GitEmail` with no `source_id` lands on `LocalGit`
  // (git email is only meaningful for the local-git connector at
  // v0.1); a `GitLabUserId`/`GitLabUsername` lands on `GitLab`; a
  // `GitHubLogin` lands on `GitHub`. If the identity is scoped to a
  // specific `source_id` we read the kind from the actual source so
  // a Jira-scoped email would render the Jira mark even though the
  // identity kind is still `GitEmail` — that path is not wired at
  // v0.1 (Jira has no identity rows yet) so we only assert the
  // natural-owner fallbacks here, which are what users see today.
  it("renders the coloured brand mark for each identity row based on the identity kind", async () => {
    const GITHUB_SOURCE: Source = {
      ...SOURCE,
      id: "src-gh",
      kind: "GitHub",
      label: "dayseam gh",
      config: { GitHub: { api_base_url: "https://api.github.com" } },
    };
    const GITLAB_SOURCE: Source = {
      ...SOURCE,
      id: "src-gl",
      kind: "GitLab",
      label: "self-hosted gl",
      config: {
        GitLab: {
          base_url: "https://gitlab.example.com",
          user_id: 42,
          username: "ada",
        },
      },
    };
    const GIT_EMAIL: SourceIdentity = {
      id: "id-git",
      person_id: "person-1",
      source_id: null,
      kind: "GitEmail",
      external_actor_id: "ada@example.com",
    };
    const GH_LOGIN: SourceIdentity = {
      id: "id-gh",
      person_id: "person-1",
      source_id: null,
      kind: "GitHubLogin",
      external_actor_id: "ada-gh",
    };
    const GL_USER: SourceIdentity = {
      id: "id-gl",
      person_id: "person-1",
      source_id: "src-gl",
      kind: "GitLabUsername",
      external_actor_id: "ada-gl",
    };
    registerInvokeHandler(
      "sources_list",
      async () => [SOURCE, GITHUB_SOURCE, GITLAB_SOURCE],
    );
    registerInvokeHandler(
      "identities_list_for",
      async () => [GIT_EMAIL, GH_LOGIN, GL_USER],
    );

    render(<IdentityManagerDialog open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByTestId("identity-row-id-git")).toBeInTheDocument(),
    );

    const gitRow = screen.getByTestId("identity-row-id-git");
    expect(
      within(gitRow).getByTestId("connector-logo-LocalGit"),
    ).toBeInTheDocument();
    expect(
      within(gitRow).getByTestId("connector-logo-LocalGit"),
    ).toHaveAttribute("data-colored", "true");

    const ghRow = screen.getByTestId("identity-row-id-gh");
    expect(
      within(ghRow).getByTestId("connector-logo-GitHub"),
    ).toBeInTheDocument();

    const glRow = screen.getByTestId("identity-row-id-gl");
    expect(
      within(glRow).getByTestId("connector-logo-GitLab"),
    ).toBeInTheDocument();
  });

  it("degrades to an error banner when persons_get_self fails", async () => {
    registerInvokeHandler("persons_get_self", async () => {
      throw new Error("db locked");
    });
    render(<IdentityManagerDialog open onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(/could not load the self-person/i)).toBeInTheDocument(),
    );
  });
});
