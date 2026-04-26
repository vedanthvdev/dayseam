//! Minimal `tid` extractor for Microsoft Graph access tokens.
//!
//! Microsoft's `/oauth2/v2.0/token` endpoint issues access tokens
//! that are JWS-packed JWTs with three `.`-separated base64url
//! segments (header.payload.signature). DAY-203 needs the `tid`
//! (tenant id) claim from the payload to record the Outlook source's
//! identity before Graph's `/me` probe (the probe itself validates
//! the token — this function only reads claims out of it).
//!
//! We deliberately *do not* verify the signature here: the access
//! token was just minted by the PKCE token-exchange that
//! [`apps/desktop/src-tauri/src/ipc/oauth.rs`](apps/desktop/src-tauri/src/ipc/oauth.rs)
//! ran over our own TLS-validated connection. Verifying against
//! Microsoft's public JWKS would re-validate us against ourselves
//! and pull in a `jsonwebtoken` dependency for no additional
//! security guarantee. Any tampering between the token endpoint and
//! this function would require the attacker to already own our
//! process memory. The subsequent Graph `/me` call is still the
//! final authority on whether the token is good.
//!
//! The extractor is intentionally liberal with failure modes
//! (missing segments, malformed base64, non-JSON payload, missing
//! `tid` field) and collapses them all into a single
//! [`OUTLOOK_TENANT_UNRESOLVED`][code] code. Distinguishing "bad
//! base64" from "missing tid" in user-facing copy would just surface
//! Microsoft's implementation detail — what the user needs to know
//! is "we couldn't figure out which tenant you signed into", which
//! always maps to the same remediation: retry the sign-in.
//!
//! [code]: dayseam_core::error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use dayseam_core::{error_codes, DayseamError};
use serde::Deserialize;

/// Parse the JWT access token and return its `tid` (tenant id)
/// claim.
///
/// On any failure (wrong segment count, malformed base64, non-JSON
/// payload, missing/empty `tid`) returns
/// [`DayseamError::InvalidConfig`] tagged with
/// [`IPC_OUTLOOK_TENANT_UNRESOLVED`][error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED].
/// The IPC layer turns that into an inline "couldn't read the
/// tenant from your sign-in" error the user resolves by clicking
/// "Sign in as a different account".
pub fn extract_tid(access_token: &str) -> Result<String, DayseamError> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() != 3 {
        return Err(tenant_error("access token is not a three-segment JWT"));
    }
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|err| tenant_error(format!("JWT payload base64url decode failed: {err}")))?;

    #[derive(Deserialize)]
    struct Claims {
        tid: Option<String>,
    }
    let claims: Claims = serde_json::from_slice(&payload_bytes)
        .map_err(|err| tenant_error(format!("JWT payload is not valid JSON: {err}")))?;

    match claims.tid {
        Some(tid) if !tid.is_empty() => Ok(tid),
        _ => Err(tenant_error("JWT payload has no non-empty `tid` claim")),
    }
}

fn tenant_error(message: impl Into<String>) -> DayseamError {
    DayseamError::InvalidConfig {
        code: error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_jwt(payload_json: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(b"not-verified");
        format!("{header}.{payload}.{signature}")
    }

    #[test]
    fn extract_tid_reads_tenant_from_payload() {
        let token = make_jwt(r#"{"tid":"11111111-2222-3333-4444-555555555555","sub":"x"}"#);
        let tid = extract_tid(&token).expect("tid present");
        assert_eq!(tid, "11111111-2222-3333-4444-555555555555");
    }

    #[test]
    fn extract_tid_accepts_extra_claims() {
        let token = make_jwt(
            r#"{"aud":"https://graph.microsoft.com","iss":"https://sts.windows.net/abc/","tid":"abc","unused":true}"#,
        );
        assert_eq!(extract_tid(&token).unwrap(), "abc");
    }

    #[test]
    fn extract_tid_rejects_non_jwt_shape() {
        let err = extract_tid("not-a-jwt").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[test]
    fn extract_tid_rejects_two_segment_token() {
        let err = extract_tid("aaa.bbb").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[test]
    fn extract_tid_rejects_non_base64_payload() {
        let err = extract_tid("aaa.!!!.ccc").unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[test]
    fn extract_tid_rejects_non_json_payload() {
        let payload = URL_SAFE_NO_PAD.encode(b"not json");
        let token = format!("aaa.{payload}.ccc");
        let err = extract_tid(&token).unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[test]
    fn extract_tid_rejects_missing_claim() {
        let token = make_jwt(r#"{"sub":"only-sub","aud":"x"}"#);
        let err = extract_tid(&token).unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }

    #[test]
    fn extract_tid_rejects_empty_claim() {
        let token = make_jwt(r#"{"tid":""}"#);
        let err = extract_tid(&token).unwrap_err();
        assert_eq!(err.code(), error_codes::IPC_OUTLOOK_TENANT_UNRESOLVED);
    }
}
