import { describe, expect, it } from "vitest";
import { GITLAB_ERROR_CODES } from "@dayseam/ipc-types";
import { gitlabErrorCopy } from "../gitlabErrorCopy";

describe("gitlabErrorCopy parity", () => {
  it("every_gitlab_error_code_has_copy", () => {
    // `GITLAB_ERROR_CODES` is regenerated from
    // `dayseam_core::error_codes::ALL` by the Rust
    // `ts_types_generated` test, so this test fails whenever a new
    // `gitlab.*` code lands in the Rust source of truth without a
    // matching entry here.
    for (const code of GITLAB_ERROR_CODES) {
      expect(
        gitlabErrorCopy[code],
        `missing gitlabErrorCopy entry for ${code}`,
      ).toBeDefined();
      expect(gitlabErrorCopy[code]?.title).toBeTruthy();
      expect(gitlabErrorCopy[code]?.body).toBeTruthy();
    }
  });

  it("does not carry stale entries for retired codes", () => {
    // If a code is removed from Rust, the map should shrink with it.
    const known = new Set<string>(GITLAB_ERROR_CODES);
    for (const key of Object.keys(gitlabErrorCopy)) {
      expect(known.has(key), `stale gitlabErrorCopy entry: ${key}`).toBe(true);
    }
  });
});
