// Normalisation table for the Atlassian workspace-URL field. Holds
// the DAY-82 invariant `workspace_url_normalisation`: whatever the
// user types, the dialog's submit path stores exactly one of these
// shapes on the `SourceConfig::{Jira, Confluence}.workspace_url` row
// — never a silently-upgraded one, never a path-appended one.

import { describe, expect, it } from "vitest";
import {
  atlassianTokenPageUrl,
  normaliseWorkspaceUrl,
} from "../atlassian-workspace-url";

describe("normaliseWorkspaceUrl", () => {
  it("expands a bare slug to the canonical atlassian.net URL", () => {
    expect(normaliseWorkspaceUrl("company")).toEqual({
      kind: "ok",
      url: "https://company.atlassian.net",
    });
  });

  it("accepts the canonical shape as-is", () => {
    expect(
      normaliseWorkspaceUrl("https://company.atlassian.net"),
    ).toEqual({ kind: "ok", url: "https://company.atlassian.net" });
  });

  it("strips a trailing slash", () => {
    expect(
      normaliseWorkspaceUrl("https://company.atlassian.net/"),
    ).toEqual({ kind: "ok", url: "https://company.atlassian.net" });
  });

  it("rejects http:// (Atlassian Cloud is https-only)", () => {
    const result = normaliseWorkspaceUrl("http://company.atlassian.net");
    expect(result.kind).toBe("invalid");
  });

  it("rejects input with a path segment", () => {
    const result = normaliseWorkspaceUrl(
      "https://company.atlassian.net/wiki",
    );
    expect(result.kind).toBe("invalid");
  });

  it("rejects non-http(s) schemes", () => {
    expect(normaliseWorkspaceUrl("ftp://company.atlassian.net").kind)
      .toBe("invalid");
    expect(normaliseWorkspaceUrl("javascript:alert(1)").kind).toBe("invalid");
  });

  it("rejects input with a query string or fragment", () => {
    expect(
      normaliseWorkspaceUrl("https://company.atlassian.net?x=1").kind,
    ).toBe("invalid");
    expect(
      normaliseWorkspaceUrl("https://company.atlassian.net#abc").kind,
    ).toBe("invalid");
  });

  it("treats empty input as empty (submit stays disabled)", () => {
    expect(normaliseWorkspaceUrl("")).toEqual({ kind: "empty" });
    expect(normaliseWorkspaceUrl("   ")).toEqual({ kind: "empty" });
  });

  it("rejects nonsense strings with special characters", () => {
    expect(normaliseWorkspaceUrl("my workspace").kind).toBe("invalid");
    expect(normaliseWorkspaceUrl("company!").kind).toBe("invalid");
  });

  // DOG-v0.2-03 (security). The pre-fix dialog accepted any host
  // that parsed as a URL — pasting `example.com` or
  // `attacker.example` would shape into a perfectly stored
  // `SourceConfig::*.workspace_url`, and the next
  // `atlassian_validate_credentials` round-trip would post the
  // user's API token to that origin. The post-fix rule: only hosts
  // under the `.atlassian.net` apex are accepted.
  it("rejects hosts outside the .atlassian.net apex (security)", () => {
    expect(normaliseWorkspaceUrl("example.com").kind).toBe("invalid");
    expect(normaliseWorkspaceUrl("https://attacker.example/").kind).toBe(
      "invalid",
    );
    expect(
      normaliseWorkspaceUrl("https://acme.atlassian.net.attacker.example/")
        .kind,
    ).toBe("invalid");
    expect(
      normaliseWorkspaceUrl("https://atlassian.net.attacker.example/").kind,
    ).toBe("invalid");
  });

  it("accepts the .atlassian.net apex itself, case-insensitively", () => {
    expect(normaliseWorkspaceUrl("https://Acme.Atlassian.NET").kind).toBe(
      "ok",
    );
    expect(normaliseWorkspaceUrl("https://atlassian.net").kind).toBe("ok");
  });
});

describe("atlassianTokenPageUrl", () => {
  it("returns the canonical id.atlassian.com API-tokens page", () => {
    expect(atlassianTokenPageUrl()).toBe(
      "https://id.atlassian.com/manage-profile/security/api-tokens",
    );
  });
});
