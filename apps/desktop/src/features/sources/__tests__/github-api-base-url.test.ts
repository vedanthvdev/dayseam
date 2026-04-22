// Acceptance table for `normaliseGithubApiBaseUrl`. Mirrors the Rust
// `parse_api_base_url` tests in `apps/desktop/src-tauri/src/ipc/
// github.rs` so a change to one side drags the other along.

import { describe, expect, it } from "vitest";
import {
  GITHUB_CLOUD_API_BASE_URL,
  normaliseGithubApiBaseUrl,
  tokenPageUrl,
} from "../github-api-base-url";

describe("normaliseGithubApiBaseUrl", () => {
  it("treats whitespace-only input as empty", () => {
    expect(normaliseGithubApiBaseUrl("")).toEqual({ kind: "empty" });
    expect(normaliseGithubApiBaseUrl("   ")).toEqual({ kind: "empty" });
  });

  it("normalises a bare host to https + trailing slash", () => {
    expect(normaliseGithubApiBaseUrl("api.github.com")).toEqual({
      kind: "ok",
      url: "https://api.github.com/",
      isCloud: true,
    });
  });

  it("is a no-op when the input is already canonical", () => {
    expect(normaliseGithubApiBaseUrl("https://api.github.com/")).toEqual({
      kind: "ok",
      url: "https://api.github.com/",
      isCloud: true,
    });
  });

  it("appends a trailing slash when missing", () => {
    expect(normaliseGithubApiBaseUrl("https://api.github.com")).toEqual({
      kind: "ok",
      url: "https://api.github.com/",
      isCloud: true,
    });
  });

  it("preserves an Enterprise /api/v3 path with trailing slash", () => {
    expect(normaliseGithubApiBaseUrl("https://ghe.acme.com/api/v3")).toEqual({
      kind: "ok",
      url: "https://ghe.acme.com/api/v3/",
      isCloud: false,
    });
  });

  it("rejects http:// loudly rather than upgrading silently", () => {
    const n = normaliseGithubApiBaseUrl("http://api.github.com/");
    expect(n.kind).toBe("invalid");
    if (n.kind === "invalid") {
      expect(n.reason).toMatch(/https/i);
    }
  });

  it("rejects input with a query string", () => {
    const n = normaliseGithubApiBaseUrl("https://api.github.com/?x=1");
    expect(n.kind).toBe("invalid");
  });

  it("rejects input with a fragment", () => {
    const n = normaliseGithubApiBaseUrl("https://api.github.com/#x");
    expect(n.kind).toBe("invalid");
  });

  it("rejects garbage that doesn't parse as a URL", () => {
    const n = normaliseGithubApiBaseUrl("::::not a url::::");
    expect(n.kind).toBe("invalid");
  });

  it("exposes the cloud base URL as the default prefill value", () => {
    // The dialog prefills this string verbatim, so if someone ever
    // changes the constant in a way that breaks normalisation the
    // test fails here rather than silently in a rendering snapshot.
    expect(normaliseGithubApiBaseUrl(GITHUB_CLOUD_API_BASE_URL)).toMatchObject({
      kind: "ok",
      url: GITHUB_CLOUD_API_BASE_URL,
      isCloud: true,
    });
  });
});

describe("tokenPageUrl", () => {
  it("points at github.com for cloud tenants", () => {
    const url = tokenPageUrl("https://api.github.com/");
    expect(url).toMatch(/^https:\/\/github\.com\/settings\/tokens\/new\?/);
    expect(url).toMatch(/description=Dayseam/);
    // The exact scope set matches the plan doc — encoded once by
    // URLSearchParams so the commas come across as `%2C`.
    expect(url).toMatch(/scopes=repo%2Cread%3Aorg%2Cread%3Auser/);
  });

  it("keeps the Enterprise host and drops the api/v3 suffix", () => {
    const url = tokenPageUrl("https://ghe.acme.com/api/v3/");
    expect(url).toMatch(/^https:\/\/ghe\.acme\.com\/settings\/tokens\/new\?/);
  });

  it("falls back to the raw input when the URL is unparseable", () => {
    const raw = "::::not a url::::";
    expect(tokenPageUrl(raw)).toBe(raw);
  });
});
