import { describe, expect, it } from "vitest";
import { OUTLOOK_ERROR_CODES } from "@dayseam/ipc-types";
import { outlookErrorCopy } from "../outlookErrorCopy";

describe("outlookErrorCopy parity", () => {
  it("every outlook error code has copy", () => {
    // `OUTLOOK_ERROR_CODES` is regenerated from
    // `dayseam_core::error_codes::ALL` by the Rust
    // `ts_types_generated` test, so this test fails whenever a new
    // `outlook.*` / `ipc.outlook.*` code lands in the Rust source of
    // truth without a matching entry here.
    for (const code of OUTLOOK_ERROR_CODES) {
      expect(
        outlookErrorCopy[code],
        `missing outlookErrorCopy entry for ${code}`,
      ).toBeDefined();
      expect(outlookErrorCopy[code]?.title).toBeTruthy();
      expect(outlookErrorCopy[code]?.body).toBeTruthy();
    }
  });

  it("does not carry stale entries for retired codes", () => {
    const known = new Set<string>(OUTLOOK_ERROR_CODES);
    for (const key of Object.keys(outlookErrorCopy)) {
      expect(known.has(key), `stale outlookErrorCopy entry: ${key}`).toBe(true);
    }
  });

  it("consent_required carries an admin-consent URL", () => {
    // The dialog (and the health card) branches on this URL to
    // render an "open consent URL" affordance that routes the user
    // to their IT admin's endpoint; the entry must exist so the
    // action is always visible for this error.
    expect(outlookErrorCopy["outlook.consent_required"].adminConsentUrl).toMatch(
      /^https:\/\/login\.microsoftonline\.com\//,
    );
  });
});
