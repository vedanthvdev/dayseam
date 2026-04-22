// RTL coverage for `SourceErrorCard` — one test per `gitlab.*` code
// from `dayseam_core::error_codes::ALL` so any copy drift is caught
// at the render layer, not just at the parity level. The card's job
// is narrow: render the headline + body from `gitlabErrorCopy`, and
// render a "Reconnect" button iff the code's `action` is
// `"reconnect"`. Auth codes hit that button; everything else stays
// informational.
//
// Unknown codes fall through to a generic headline so a future
// non-gitlab.* code (e.g. `local_git.*`, or a yet-unmapped upstream
// code) still renders something the user can triage.

import { render, screen, fireEvent } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { DayseamError, Source } from "@dayseam/ipc-types";
import {
  ATLASSIAN_ERROR_CODES,
  GITHUB_ERROR_CODES,
  GITLAB_ERROR_CODES,
} from "@dayseam/ipc-types";
import { SourceErrorCard } from "../SourceErrorCard";
import { atlassianErrorCopy } from "../atlassianErrorCopy";
import { githubErrorCopy } from "../githubErrorCopy";
import { gitlabErrorCopy } from "../gitlabErrorCopy";

const GITLAB_SOURCE: Source = {
  id: "gitlab-1",
  kind: "GitLab",
  label: "Acme GitLab",
  config: {
    GitLab: {
      base_url: "https://gitlab.acme.test",
      user_id: 42,
      username: "ved",
    },
  },
  secret_ref: {
    keychain_service: "dayseam",
    keychain_account: "gitlab.gitlab-1",
  },
  created_at: "2026-04-17T12:00:00Z",
  last_sync_at: null,
  last_health: { ok: true, checked_at: null, last_error: null },
};

function errorFor(code: string): DayseamError {
  // The card only reads `error.data.code`; every variant exposes it,
  // so we pick `Auth` here purely because `gitlab.auth.*` codes are
  // the most common callers.
  return {
    variant: "Auth",
    data: { code, message: "simulated", retryable: false, action_hint: null },
  };
}

describe("SourceErrorCard", () => {
  it.each(GITLAB_ERROR_CODES.map((c) => [c]))(
    "renders the copy entry for %s",
    (code) => {
      const onReconnect = vi.fn();
      render(
        <SourceErrorCard
          source={GITLAB_SOURCE}
          error={errorFor(code)}
          onReconnect={onReconnect}
        />,
      );
      const copy = gitlabErrorCopy[code];
      expect(screen.getByText(copy.title)).toBeInTheDocument();
      expect(screen.getByText(copy.body)).toBeInTheDocument();
      expect(screen.getByText(code)).toBeInTheDocument();

      const reconnect = screen.queryByRole("button", { name: /reconnect/i });
      if (copy.action === "reconnect") {
        expect(reconnect).not.toBeNull();
      } else {
        expect(reconnect).toBeNull();
      }
    },
  );

  it("Reconnect button fires onReconnect with the source for auth codes", () => {
    const onReconnect = vi.fn();
    render(
      <SourceErrorCard
        source={GITLAB_SOURCE}
        error={errorFor("gitlab.auth.invalid_token")}
        onReconnect={onReconnect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /reconnect/i }));
    expect(onReconnect).toHaveBeenCalledTimes(1);
    expect(onReconnect).toHaveBeenCalledWith(GITLAB_SOURCE);
  });

  it.each(ATLASSIAN_ERROR_CODES.map((c) => [c]))(
    "renders the atlassian copy entry for %s",
    (code) => {
      const onReconnect = vi.fn();
      render(
        <SourceErrorCard
          source={GITLAB_SOURCE}
          error={errorFor(code)}
          onReconnect={onReconnect}
        />,
      );
      const copy = atlassianErrorCopy[code];
      expect(screen.getByText(copy.title)).toBeInTheDocument();
      expect(screen.getByText(copy.body)).toBeInTheDocument();
      expect(screen.getByText(code)).toBeInTheDocument();

      const reconnect = screen.queryByRole("button", { name: /reconnect/i });
      if (copy.action === "reconnect") {
        expect(reconnect).not.toBeNull();
      } else {
        expect(reconnect).toBeNull();
      }
    },
  );

  it.each(GITHUB_ERROR_CODES.map((c) => [c]))(
    "renders the github copy entry for %s",
    (code) => {
      const onReconnect = vi.fn();
      render(
        <SourceErrorCard
          source={GITLAB_SOURCE}
          error={errorFor(code)}
          onReconnect={onReconnect}
        />,
      );
      const copy = githubErrorCopy[code];
      expect(screen.getByText(copy.title)).toBeInTheDocument();
      expect(screen.getByText(copy.body)).toBeInTheDocument();
      expect(screen.getByText(code)).toBeInTheDocument();

      const reconnect = screen.queryByRole("button", { name: /reconnect/i });
      if (copy.action === "reconnect") {
        expect(reconnect).not.toBeNull();
      } else {
        expect(reconnect).toBeNull();
      }
    },
  );

  it("Reconnect fires for github auth codes", () => {
    const onReconnect = vi.fn();
    render(
      <SourceErrorCard
        source={GITLAB_SOURCE}
        error={errorFor("github.auth.invalid_credentials")}
        onReconnect={onReconnect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /reconnect/i }));
    expect(onReconnect).toHaveBeenCalledTimes(1);
    expect(onReconnect).toHaveBeenCalledWith(GITLAB_SOURCE);
  });

  it("Reconnect fires for atlassian auth codes", () => {
    const onReconnect = vi.fn();
    render(
      <SourceErrorCard
        source={GITLAB_SOURCE}
        error={errorFor("atlassian.auth.invalid_credentials")}
        onReconnect={onReconnect}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /reconnect/i }));
    expect(onReconnect).toHaveBeenCalledTimes(1);
    expect(onReconnect).toHaveBeenCalledWith(GITLAB_SOURCE);
  });

  it("falls back to a generic headline for unmapped codes", () => {
    render(
      <SourceErrorCard
        source={GITLAB_SOURCE}
        error={errorFor("not.a.real.code")}
        onReconnect={vi.fn()}
      />,
    );
    expect(screen.getByText("Something went wrong")).toBeInTheDocument();
    expect(screen.getByText("not.a.real.code")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /reconnect/i }),
    ).toBeNull();
  });
});
