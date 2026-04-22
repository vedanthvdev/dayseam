import { describe, expect, it } from "vitest";
import { GITHUB_ERROR_CODES } from "@dayseam/ipc-types";
import { githubErrorCopy } from "../githubErrorCopy";

describe("githubErrorCopy parity", () => {
  it("every github error code has copy", () => {
    // `GITHUB_ERROR_CODES` is regenerated from
    // `dayseam_core::error_codes::ALL` by the Rust
    // `ts_types_generated` test, so this test fails whenever a new
    // `github.*` code lands in the Rust source of truth without a
    // matching entry here.
    for (const code of GITHUB_ERROR_CODES) {
      expect(
        githubErrorCopy[code],
        `missing githubErrorCopy entry for ${code}`,
      ).toBeDefined();
      expect(githubErrorCopy[code]?.title).toBeTruthy();
      expect(githubErrorCopy[code]?.body).toBeTruthy();
    }
  });

  it("does not carry stale entries for retired codes", () => {
    const known = new Set<string>(GITHUB_ERROR_CODES);
    for (const key of Object.keys(githubErrorCopy)) {
      expect(known.has(key), `stale githubErrorCopy entry: ${key}`).toBe(true);
    }
  });
});
