//! Short-lived signed URLs for finished documents.
//!
//! A browser cannot send an `Authorization` header on a plain link, so a document URL
//! has to carry its own proof. It must not carry the API key: that would put a
//! long-lived secret into access logs, browser history and every `Referer` the PDF
//! generates. A signature over `(tenant, id, expiry)` is clickable, and leaking one
//! costs a single document for a few minutes rather than the key.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64URL;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use thiserror::Error;

use crate::principal::TenantId;

/// Bytes of HMAC output kept. 128 bits is far beyond forgeable while keeping URLs short.
const SIGNATURE_BYTES: usize = 16;

/// Domain separator and version.
///
/// Prefixing the signed message means a signature minted here can never be replayed
/// into some future context that also signs with this secret — and the `v1` gives a
/// way to rotate the scheme without honouring old signatures.
const DOMAIN: &str = "typst-mcp/file/v1";

/// Why a signature was not accepted.
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
pub enum SignatureError {
    #[error("this link has expired; request a fresh one")]
    Expired,
    #[error("the link's signature is not valid")]
    Invalid,
    #[error("the link is missing its expiry or signature")]
    Missing,
}

/// Mints and checks document-link signatures.
#[derive(Clone)]
pub struct Signer {
    secret: Vec<u8>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").finish_non_exhaustive()
    }
}

impl Signer {
    pub fn new(secret: impl Into<Vec<u8>>) -> Self {
        Self {
            secret: secret.into(),
        }
    }

    /// Sign `(tenant, id)` until `expires_at` (Unix seconds).
    pub fn sign(&self, tenant: &TenantId, id: &str, expires_at: u64) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.secret)
            .expect("HMAC accepts keys of any length");
        mac.update(message(tenant, id, expires_at).as_bytes());
        BASE64URL.encode(&mac.finalize().into_bytes()[..SIGNATURE_BYTES])
    }

    /// Mint a signature valid for `ttl_seconds` from now.
    pub fn sign_for(&self, tenant: &TenantId, id: &str, ttl_seconds: u64) -> (String, u64) {
        let expires_at = now() + ttl_seconds;
        (self.sign(tenant, id, expires_at), expires_at)
    }

    /// Check a presented signature.
    pub fn verify(
        &self,
        tenant: &TenantId,
        id: &str,
        expires_at: u64,
        presented: &str,
    ) -> Result<(), SignatureError> {
        // Expiry first: an expired link is a distinct, actionable outcome, and there is
        // no reason to spend a comparison on it.
        if expires_at <= now() {
            return Err(SignatureError::Expired);
        }
        let expected = self.sign(tenant, id, expires_at);
        // Constant-time: a byte-by-byte comparison would let an attacker recover a
        // valid signature one character at a time.
        if expected.as_bytes().ct_eq(presented.as_bytes()).into() {
            Ok(())
        } else {
            Err(SignatureError::Invalid)
        }
    }

    /// Verify from raw query parameters.
    pub fn verify_params(
        &self,
        tenant: &TenantId,
        id: &str,
        expires_at: Option<&str>,
        signature: Option<&str>,
    ) -> Result<(), SignatureError> {
        let (Some(exp), Some(sig)) = (expires_at, signature) else {
            return Err(SignatureError::Missing);
        };
        let exp: u64 = exp.parse().map_err(|_| SignatureError::Invalid)?;
        self.verify(tenant, id, exp, sig)
    }
}

/// The exact bytes that get signed.
///
/// Fields are joined with a separator that cannot occur in any of them, so no two
/// distinct tuples can produce the same message.
fn message(tenant: &TenantId, id: &str, expires_at: u64) -> String {
    format!("{DOMAIN}|{tenant}|{id}|{expires_at}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signer() -> Signer {
        Signer::new(b"a-signing-secret-of-at-least-32-bytes".to_vec())
    }

    fn tenant() -> TenantId {
        TenantId::parse("0123456789abcdef").expect("valid id")
    }

    #[test]
    fn a_fresh_signature_verifies() {
        let s = signer();
        let (sig, exp) = s.sign_for(&tenant(), "01JOB", 900);
        assert_eq!(s.verify(&tenant(), "01JOB", exp, &sig), Ok(()));
    }

    #[test]
    fn an_expired_signature_is_refused_even_though_it_is_authentic() {
        let s = signer();
        let past = now() - 1;
        let sig = s.sign(&tenant(), "01JOB", past);
        assert_eq!(
            s.verify(&tenant(), "01JOB", past, &sig),
            Err(SignatureError::Expired)
        );
    }

    #[test]
    fn tampering_with_any_field_invalidates_it() {
        let s = signer();
        let (sig, exp) = s.sign_for(&tenant(), "01JOB", 900);
        let other = TenantId::parse("fedcba9876543210").expect("valid");

        // A different tenant is the important one: it is what stops a leaked link from
        // reaching into someone else's storage.
        assert_eq!(
            s.verify(&other, "01JOB", exp, &sig),
            Err(SignatureError::Invalid)
        );
        assert_eq!(
            s.verify(&tenant(), "01OTHER", exp, &sig),
            Err(SignatureError::Invalid)
        );
        // Extending the expiry must not be possible without re-signing.
        assert_eq!(
            s.verify(&tenant(), "01JOB", exp + 1, &sig),
            Err(SignatureError::Invalid)
        );
        assert_eq!(
            s.verify(&tenant(), "01JOB", exp, "AAAAAAAAAAAAAAAAAAAAAA"),
            Err(SignatureError::Invalid)
        );
    }

    #[test]
    fn a_different_secret_does_not_verify() {
        let (sig, exp) = signer().sign_for(&tenant(), "01JOB", 900);
        let other = Signer::new(b"a-completely-different-secret-value!!".to_vec());
        assert_eq!(
            other.verify(&tenant(), "01JOB", exp, &sig),
            Err(SignatureError::Invalid)
        );
    }

    #[test]
    fn field_boundaries_cannot_be_shifted() {
        // Without a separator that cannot appear in the fields, ("ab", "c") and
        // ("a", "bc") would sign the same bytes.
        let s = signer();
        let exp = now() + 900;
        assert_ne!(s.sign(&tenant(), "ab|c", exp), s.sign(&tenant(), "ab", exp));
    }

    #[test]
    fn missing_parameters_are_reported_as_missing() {
        let s = signer();
        assert_eq!(
            s.verify_params(&tenant(), "01JOB", None, Some("sig")),
            Err(SignatureError::Missing)
        );
        assert_eq!(
            s.verify_params(&tenant(), "01JOB", Some("123"), None),
            Err(SignatureError::Missing)
        );
    }

    #[test]
    fn an_unparsable_expiry_is_invalid_not_a_panic() {
        let s = signer();
        for exp in ["", "abc", "-1", "99999999999999999999999999"] {
            assert_eq!(
                s.verify_params(&tenant(), "01JOB", Some(exp), Some("sig")),
                Err(SignatureError::Invalid),
                "{exp:?}"
            );
        }
    }

    #[test]
    fn signatures_are_url_safe() {
        let (sig, _) = signer().sign_for(&tenant(), "01JOB", 900);
        assert!(
            sig.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "signature needs escaping in a URL: {sig}"
        );
    }

    #[test]
    fn the_signer_never_prints_its_secret() {
        let rendered = format!("{:?}", signer());
        assert!(!rendered.contains("signing-secret"), "{rendered}");
    }
}
