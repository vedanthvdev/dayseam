// Normalisation table for the GitLab base-URL field. Holds the Task 3
// invariant `base_url_normalisation_table`: whatever the user types,
// the dialog's submit path stores exactly one of these shapes on the
// `SourceConfig::GitLab.base_url` row — never a silently-upgraded one.

import { describe, expect, it } from "vitest";
import { normaliseBaseUrl, tokenPageUrl } from "../base-url";

describe("normaliseBaseUrl", () => {
  it("prefills https:// when the scheme is missing", () => {
    const result = normaliseBaseUrl("gitlab.example.com");
    expect(result).toEqual({
      kind: "ok",
      url: "https://gitlab.example.com",
      insecure: false,
    });
  });

  it("accepts https:// as-is", () => {
    const result = normaliseBaseUrl("https://gitlab.example.com");
    expect(result).toEqual({
      kind: "ok",
      url: "https://gitlab.example.com",
      insecure: false,
    });
  });

  it("keeps http:// but flags it as insecure (no silent upgrade)", () => {
    const result = normaliseBaseUrl("http://gitlab.example.com");
    expect(result).toEqual({
      kind: "ok",
      url: "http://gitlab.example.com",
      insecure: true,
    });
  });

  it("strips a trailing slash", () => {
    const result = normaliseBaseUrl("gitlab.example.com/");
    expect(result).toEqual({
      kind: "ok",
      url: "https://gitlab.example.com",
      insecure: false,
    });
  });

  it("rejects input with a path component", () => {
    const result = normaliseBaseUrl("gitlab.example.com/path");
    expect(result.kind).toBe("invalid");
  });

  it("treats empty input as empty (submit stays disabled)", () => {
    expect(normaliseBaseUrl("")).toEqual({ kind: "empty" });
    expect(normaliseBaseUrl("   ")).toEqual({ kind: "empty" });
  });

  it("rejects non-http(s) schemes", () => {
    expect(normaliseBaseUrl("ftp://gitlab.example.com").kind).toBe("invalid");
    expect(normaliseBaseUrl("javascript:alert(1)").kind).toBe("invalid");
  });

  it("rejects query strings and fragments", () => {
    expect(normaliseBaseUrl("https://gitlab.example.com?x=1").kind).toBe(
      "invalid",
    );
    expect(normaliseBaseUrl("https://gitlab.example.com#top").kind).toBe(
      "invalid",
    );
  });

  it("accepts a port", () => {
    expect(normaliseBaseUrl("https://gitlab.example.com:8443")).toEqual({
      kind: "ok",
      url: "https://gitlab.example.com:8443",
      insecure: false,
    });
  });
});

describe("tokenPageUrl", () => {
  it("appends the personal-access-tokens path with Dayseam scopes", () => {
    expect(tokenPageUrl("https://gitlab.example.com")).toBe(
      "https://gitlab.example.com/-/user_settings/personal_access_tokens?name=Dayseam&scopes=read_api,read_user",
    );
  });

  it("tolerates a trailing slash on the base URL", () => {
    expect(tokenPageUrl("https://gitlab.example.com/")).toBe(
      "https://gitlab.example.com/-/user_settings/personal_access_tokens?name=Dayseam&scopes=read_api,read_user",
    );
  });
});
