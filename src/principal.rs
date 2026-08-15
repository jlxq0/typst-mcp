//! Who is calling, and where their data lives.
//!
//! Two ways in — an OIDC token on `/mcp`, a static key on `/api/v1` — and one
//! `Principal` out, so nothing downstream has to care which door was used.
//!
//! Storage is partitioned by a tenant id derived from the caller's identity. The
//! derivation is an HMAC rather than a bare hash so that an identifier appearing in a
//! log does not also reveal the directory it maps to.

use std::fmt;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// Length of a derived tenant id, in base32 characters.
const TENANT_LEN: usize = 16;

/// Crockford base32 without the ambiguous letters, so an id can be read aloud and
/// typed back without I/L/O/U confusion.
const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// A person, authenticated by an OIDC token.
    User {
        /// The provider's stable subject id.
        subject: String,
        /// The issuer, so two providers cannot collide on a subject.
        issuer: String,
        name: Option<String>,
    },
    /// A service, authenticated by a static API key.
    Service {
        /// The key's label from configuration, e.g. `cardmunk`.
        label: String,
    },
}

impl Principal {
    /// The string the tenant id is derived from.
    ///
    /// Issuer and subject are joined with a separator that cannot appear in either, so
    /// no two identities can produce the same input by concatenation.
    fn identity(&self) -> String {
        match self {
            Self::User {
                subject, issuer, ..
            } => format!("oidc\u{0}{issuer}\u{0}{subject}"),
            Self::Service { label } => format!("key\u{0}{label}"),
        }
    }

    /// How this caller appears in logs and metrics. Never a credential.
    pub fn display(&self) -> &str {
        match self {
            Self::User { subject, .. } => subject,
            Self::Service { label } => label,
        }
    }

    /// Derive this caller's tenant id under `salt`.
    pub fn tenant(&self, salt: &[u8]) -> TenantId {
        TenantId::derive(salt, self.identity().as_bytes())
    }
}

/// An opaque, stable storage partition id.
///
/// Constructed only by derivation or by parsing something already in the right shape,
/// so a `TenantId` in hand is always safe to use as a path segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TenantId(String);

impl TenantId {
    /// `base32(hmac_sha256(salt, identity))`, truncated.
    ///
    /// HMAC rather than a plain hash: with a bare hash, anyone who saw a subject in a
    /// log could compute the directory it maps to. The salt keeps the mapping secret
    /// even when the input is not.
    pub fn derive(salt: &[u8], identity: &[u8]) -> Self {
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(salt).expect("HMAC accepts keys of any length");
        mac.update(identity);
        let digest = mac.finalize().into_bytes();

        // 5 bits per character, taken from the leading bytes.
        let mut out = String::with_capacity(TENANT_LEN);
        let mut buffer: u16 = 0;
        let mut bits = 0u32;
        for byte in digest.iter() {
            buffer = (buffer << 8) | u16::from(*byte);
            bits += 8;
            while bits >= 5 && out.len() < TENANT_LEN {
                bits -= 5;
                let index = ((buffer >> bits) & 0x1f) as usize;
                out.push(ALPHABET[index] as char);
            }
            if out.len() == TENANT_LEN {
                break;
            }
        }
        Self(out)
    }

    /// Accept an id that arrived from outside, e.g. in a URL path.
    ///
    /// Validated against the alphabet and length *before* it is ever used to build a
    /// path, so traversal and injection are impossible by construction rather than by
    /// a check somewhere downstream.
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != TENANT_LEN {
            return None;
        }
        value
            .bytes()
            .all(|b| ALPHABET.contains(&b))
            .then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One configured API key.
#[derive(Clone)]
pub struct ApiKey {
    label: String,
    secret: Zeroizing<String>,
    fingerprint: String,
}

impl ApiKey {
    /// Parse a `label:secret` or bare `secret` entry.
    pub fn parse(entry: &str) -> Option<Self> {
        let entry = entry.trim();
        if entry.is_empty() {
            return None;
        }
        let (label, secret) = match entry.split_once(':') {
            Some((label, secret)) if !label.is_empty() && !secret.is_empty() => (label, secret),
            // A bare key still needs a stable label, and the fingerprint is the only
            // thing available that is not the secret itself.
            _ => ("", entry),
        };
        let fingerprint = fingerprint(secret);
        let label = if label.is_empty() {
            format!("key-{}", &fingerprint[..8])
        } else {
            label.to_owned()
        };
        Some(Self {
            label,
            secret: Zeroizing::new(secret.to_owned()),
            fingerprint,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// A stable, non-reversible handle for logs and metric labels.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn principal(&self) -> Principal {
        Principal::Service {
            label: self.label.clone(),
        }
    }

    /// Constant-time comparison against a presented secret.
    fn matches(&self, presented: &str) -> bool {
        // Compare digests rather than the raw bytes: `ct_eq` on slices of different
        // lengths returns early, which would leak the key's length.
        let a = Sha256::digest(self.secret.as_bytes());
        let b = Sha256::digest(presented.as_bytes());
        a.ct_eq(&b).into()
    }
}

impl fmt::Debug for ApiKey {
    /// Never prints the secret. A key that reaches a log through a stray `{:?}` is
    /// exactly as compromised as one that was printed on purpose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiKey")
            .field("label", &self.label)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// The configured set of API keys.
#[derive(Debug, Clone, Default)]
pub struct ApiKeys {
    keys: Vec<ApiKey>,
}

impl ApiKeys {
    /// Parse a comma-separated `TYPST_MCP_API_KEYS` value.
    pub fn parse(value: &str) -> Self {
        Self {
            keys: value.split(',').filter_map(ApiKey::parse).collect(),
        }
    }

    /// Find the key matching `presented`.
    ///
    /// Every entry is checked even after a match, so the time taken does not reveal
    /// which key matched or how many were configured.
    pub fn authenticate(&self, presented: &str) -> Option<&ApiKey> {
        let mut found = None;
        for key in &self.keys {
            if key.matches(presented) {
                found = Some(key);
            }
        }
        found
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn labels(&self) -> Vec<&str> {
        self.keys.iter().map(ApiKey::label).collect()
    }
}

/// A short, non-reversible handle for a secret.
pub fn fingerprint(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))[..16].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: &[u8] = b"a-test-salt-of-at-least-32-bytes!!";

    fn user(subject: &str) -> Principal {
        Principal::User {
            subject: subject.into(),
            issuer: "https://login.example.com/v2.0".into(),
            name: None,
        }
    }

    #[test]
    fn tenant_ids_are_stable_and_well_formed() {
        let id = user("abc").tenant(SALT);
        assert_eq!(id.as_str().len(), TENANT_LEN);
        assert!(id.as_str().bytes().all(|b| ALPHABET.contains(&b)), "{id}");
        // Stability across restarts is what makes a caller's data findable at all.
        assert_eq!(id, user("abc").tenant(SALT));
    }

    #[test]
    fn different_callers_get_different_tenants() {
        let ids = [
            user("abc").tenant(SALT),
            user("abd").tenant(SALT),
            Principal::Service {
                label: "abc".into(),
            }
            .tenant(SALT),
            Principal::User {
                subject: "abc".into(),
                issuer: "https://other.example.com".into(),
                name: None,
            }
            .tenant(SALT),
        ];
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "tenant ids collided: {ids:?}");
    }

    #[test]
    fn a_user_and_a_service_with_the_same_name_are_different_tenants() {
        // Without the type prefix in `identity()`, a service labelled "abc" and a user
        // whose subject is "abc" would share a directory.
        assert_ne!(
            user("abc").tenant(SALT),
            Principal::Service {
                label: "abc".into()
            }
            .tenant(SALT)
        );
    }

    #[test]
    fn concatenation_cannot_forge_another_identity() {
        // The NUL separator cannot appear in an issuer or a subject, so no pair of
        // fields can be rearranged into another identity's input.
        let a = Principal::User {
            issuer: "https://x.example".into(),
            subject: "b/c".into(),
            name: None,
        };
        let b = Principal::User {
            issuer: "https://x.example/b".into(),
            subject: "c".into(),
            name: None,
        };
        assert_ne!(a.tenant(SALT), b.tenant(SALT));
    }

    #[test]
    fn the_salt_changes_the_mapping() {
        assert_ne!(
            user("abc").tenant(SALT),
            user("abc").tenant(b"a-different-salt-value-here!!!!!")
        );
    }

    #[test]
    fn tenant_ids_from_outside_are_validated() {
        let valid = user("abc").tenant(SALT);
        assert_eq!(TenantId::parse(valid.as_str()), Some(valid));

        for hostile in [
            "../../../etc/passwd",
            "..",
            "/",
            "short",
            "0123456789abcdefg", // too long
            "0123456789abcdeI",  // ambiguous letter, not in the alphabet
            "0123456789abcde/",
            "0123456789abcde.",
            "",
        ] {
            assert_eq!(
                TenantId::parse(hostile),
                None,
                "{hostile:?} must be refused"
            );
        }
    }

    #[test]
    fn api_keys_parse_with_and_without_labels() {
        let keys = ApiKeys::parse("alice:sk_one,sk_two, bob:sk_three ");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys.labels()[0], "alice");
        // A bare key gets a stable label derived from its fingerprint, not its bytes.
        assert!(keys.labels()[1].starts_with("key-"));
        assert!(!keys.labels()[1].contains("sk_two"));
        assert_eq!(keys.labels()[2], "bob");
    }

    #[test]
    fn empty_and_malformed_entries_are_skipped() {
        let keys = ApiKeys::parse(",,alice:,:sk_x,  ,alice:sk_ok");
        // `alice:` has no secret and `:sk_x` has no label, so both fall back to the
        // bare-key branch; only genuinely empty entries disappear.
        assert!(keys.len() >= 1);
        assert!(keys.authenticate("sk_ok").is_some());
        assert!(keys.authenticate("").is_none());
    }

    #[test]
    fn authentication_accepts_only_an_exact_key() {
        let keys = ApiKeys::parse("alice:sk_secret");
        assert_eq!(
            keys.authenticate("sk_secret").map(ApiKey::label),
            Some("alice")
        );
        for wrong in ["sk_secre", "sk_secrett", "SK_SECRET", "", "sk_secret "] {
            assert!(
                keys.authenticate(wrong).is_none(),
                "{wrong:?} must not authenticate"
            );
        }
    }

    #[test]
    fn an_empty_key_set_authenticates_nothing() {
        let keys = ApiKeys::parse("");
        assert!(keys.is_empty());
        assert!(keys.authenticate("").is_none());
        assert!(keys.authenticate("anything").is_none());
    }

    #[test]
    fn keys_never_print_their_secret() {
        let keys = ApiKeys::parse("alice:sk_super_secret");
        let rendered = format!("{keys:?}");
        assert!(!rendered.contains("sk_super_secret"), "{rendered}");
        assert!(rendered.contains("alice"), "{rendered}");
    }

    #[test]
    fn fingerprints_are_short_stable_and_not_the_secret() {
        let fp = fingerprint("sk_secret");
        assert_eq!(fp.len(), 16);
        assert_eq!(fp, fingerprint("sk_secret"));
        assert_ne!(fp, fingerprint("sk_secreu"));
        assert!(!fp.contains("sk_"));
    }
}
