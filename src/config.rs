//! Configuration, read once from the environment at startup.
//!
//! Required secrets are checked here, before anything binds a port. A server that
//! boots with a missing salt and only fails on the first request is far worse than one
//! that refuses to start: the deploy looks green and the failure lands on a user.
//!
//! There are deliberately no generated defaults for the secrets. A generated tenant
//! salt would silently re-partition every caller's storage on each restart, and a
//! generated signing secret would invalidate every outstanding link — both of which
//! present as data loss rather than as the misconfiguration they are.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

use crate::principal::ApiKeys;
use crate::store::Limits;

/// Prefix for every variable this reads.
const PREFIX: &str = "TYPST_MCP_";

/// Minimum length for a secret, in bytes.
///
/// 32 bytes of entropy. Enforced rather than documented, because a placeholder like
/// `changeme` otherwise works perfectly until someone looks.
const MIN_SECRET_BYTES: usize = 32;

/// Why the process cannot start.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{PREFIX}{name} is required but not set")]
    Missing { name: &'static str },
    #[error("{PREFIX}{name} must be at least {MIN_SECRET_BYTES} bytes; it is {actual}")]
    SecretTooShort { name: &'static str, actual: usize },
    #[error("{PREFIX}{name}={value:?} is not a valid {expected}")]
    Invalid {
        name: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error(
        "no credentials configured: set {PREFIX}API_KEYS, {PREFIX}OIDC_ISSUER, or both. \
         Starting without either would expose an unauthenticated renderer."
    )]
    NoCredentials,
}

/// Microsoft Entra (or any OIDC provider) settings for the MCP endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    /// e.g. `https://login.microsoftonline.com/{tenant}/v2.0`.
    pub issuer: String,
    /// The directory GUID, checked against the token's `tid`. Without it a token from
    /// any other Entra directory would validate against the same issuer.
    pub tenant_id: Option<String>,
    /// The App ID URI or client id this server accepts as `aud`.
    pub audience: String,
    /// The scope a token must carry.
    pub scope: String,
}

/// Everything the server needs to run.
#[derive(Debug, Clone)]
pub struct Config {
    pub public_url: String,
    pub bind_addr: SocketAddr,
    pub data_dir: PathBuf,
    pub template_dir: PathBuf,
    pub font_dirs: Vec<PathBuf>,

    pub tenant_salt: Vec<u8>,
    pub signing_secret: Vec<u8>,
    pub signed_url_ttl: Duration,

    pub api_keys: ApiKeys,
    pub oidc: Option<OidcConfig>,
    /// Entra public-client id used by the DCR shim and the authorize/token proxy.
    /// Optional: health and PRM still work without it; /authorize returns 503.
    pub client_id: Option<String>,
    pub dcr_client_id: Option<String>,
    pub oauth_redirect_uris: Vec<String>,

    /// The binary to spawn for compiles. `None` re-execs this process, which is
    /// correct in production but wrong inside a test harness, where `current_exe()`
    /// is libtest rather than the server.
    pub worker_exe: Option<PathBuf>,
    pub compile_timeout: Duration,
    pub max_concurrent_compiles: usize,
    pub worker_memory_bytes: u64,

    pub max_bundle_bytes: usize,
    pub max_upload_bytes: usize,
    pub max_pages: usize,
    pub preview_max_px: u32,

    pub limits: Limits,
}

impl Config {
    /// Read from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_source(&|name| std::env::var(format!("{PREFIX}{name}")).ok())
    }

    /// Read from an arbitrary source, so this is testable without touching the
    /// process environment (which is global, and racy under a parallel test runner).
    pub fn from_source(get: &dyn Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let public_url = required(get, "PUBLIC_URL")?
            .trim_end_matches('/')
            .to_owned();
        let tenant_salt = secret(get, "TENANT_SALT")?;
        let signing_secret = secret(get, "SIGNING_SECRET")?;

        let api_keys = get("API_KEYS")
            .map(|v| ApiKeys::parse(&v))
            .unwrap_or_default();
        let oidc = match get("OIDC_ISSUER") {
            Some(issuer) if !issuer.trim().is_empty() => Some(OidcConfig {
                issuer: issuer.trim().trim_end_matches('/').to_owned(),
                tenant_id: get("OIDC_TENANT_ID").filter(|v| !v.trim().is_empty()),
                audience: required(get, "OIDC_AUDIENCE")?,
                scope: get("OIDC_SCOPE").unwrap_or_else(|| "render".into()),
            }),
            _ => None,
        };
        let client_id = get("OIDC_CLIENT_ID")
            .or_else(|| get("CLIENT_ID"))
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty());
        let dcr_client_id = get("DCR_CLIENT_ID")
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .or_else(|| client_id.clone());
        let extras = match get("OAUTH_REDIRECT_URIS").filter(|v| !v.trim().is_empty()) {
            Some(raw) => crate::oauth_redirect::parse_allowlist(&raw, "OAUTH_REDIRECT_URIS")
                .map_err(|err| ConfigError::Invalid {
                    name: "OAUTH_REDIRECT_URIS",
                    value: err.to_string(),
                    expected: "comma-separated redirect URIs",
                })?,
            None => Vec::new(),
        };
        let oauth_redirect_uris = crate::oauth_redirect::merge_allowlist(&extras).map_err(|err| {
            ConfigError::Invalid {
                name: "OAUTH_REDIRECT_URIS",
                value: err.to_string(),
                expected: "comma-separated redirect URIs",
            }
        })?;

        // Refusing to start beats starting open. A renderer with neither door
        // authenticated is an anonymous compute endpoint on the public internet.
        if api_keys.is_empty() && oidc.is_none() {
            return Err(ConfigError::NoCredentials);
        }

        Ok(Self {
            public_url,
            bind_addr: parse(get, "BIND_ADDR", "0.0.0.0:3000", "socket address")?,
            data_dir: path(get, "DATA_DIR", "/data"),
            template_dir: path(get, "TEMPLATE_DIR", "/usr/share/typst-mcp/templates"),
            font_dirs: get("FONT_DIRS")
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.split(':').map(PathBuf::from).collect())
                .unwrap_or_else(|| vec![PathBuf::from("/usr/share/fonts/typst")]),

            tenant_salt,
            signing_secret,
            signed_url_ttl: duration(get, "SIGNED_URL_TTL", 15 * 60)?,

            api_keys,
            oidc,
            client_id,
            dcr_client_id,
            oauth_redirect_uris,

            worker_exe: get("WORKER_EXE")
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from),
            compile_timeout: duration(get, "COMPILE_TIMEOUT", 20)?,
            max_concurrent_compiles: number(get, "MAX_CONCURRENT_COMPILES", default_concurrency())?,
            worker_memory_bytes: number(get, "WORKER_MEMORY_BYTES", 512 * 1024 * 1024)?,

            max_bundle_bytes: number(get, "MAX_BUNDLE_BYTES", 8 * 1024 * 1024)?,
            max_upload_bytes: number(get, "MAX_UPLOAD_BYTES", 16 * 1024 * 1024)?,
            max_pages: number(get, "MAX_PAGES", 200)?,
            preview_max_px: number(get, "PREVIEW_MAX_PX", 2000)?,

            limits: Limits {
                output_ttl: duration(get, "OUTPUT_TTL", 2 * 60 * 60)?,
                asset_ttl: duration(get, "ASSET_TTL", 24 * 60 * 60)?,
                template_ttl: duration(get, "TEMPLATE_TTL", 7 * 24 * 60 * 60)?,
                max_tenant_bytes: number(get, "MAX_TENANT_BYTES", 512 * 1024 * 1024)?,
                max_store_bytes: number(get, "MAX_STORE_BYTES", 2 * 1024 * 1024 * 1024)?,
            },
        })
    }

    /// An absolute URL under the public base.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.public_url, path.trim_start_matches('/'))
    }

    /// The canonical MCP resource identifier: `{origin}/mcp`.
    ///
    /// RFC 9728 §3.3 and the MCP authorization spec require `resource` to match the
    /// URL the client actually connects to. claude.ai tolerates a bare origin;
    /// stricter clients reject anything that is not the exact endpoint.
    pub fn mcp_resource_url(&self) -> String {
        self.url("mcp")
    }

    /// Where the RFC 9728 protected-resource metadata lives.
    ///
    /// Path-inserted form for the `{origin}/mcp` resource
    /// (`/.well-known/oauth-protected-resource/mcp`).
    pub fn metadata_url(&self) -> String {
        self.url(".well-known/oauth-protected-resource/mcp")
    }

    pub fn callback_url(&self) -> String {
        self.url("oauth/callback")
    }

    pub fn entra_authorize_url(&self) -> Option<String> {
        self.oidc.as_ref().map(|oidc| entra_oauth_url(&oidc.issuer, "authorize"))
    }

    pub fn entra_token_url(&self) -> Option<String> {
        self.oidc.as_ref().map(|oidc| entra_oauth_url(&oidc.issuer, "token"))
    }
}

fn entra_oauth_url(issuer: &str, leaf: &str) -> String {
    let issuer = issuer.trim_end_matches('/');
    let base = issuer.strip_suffix("/v2.0").unwrap_or(issuer);
    format!("{base}/oauth2/v2.0/{leaf}")
}

fn required(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    get(name)
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .ok_or(ConfigError::Missing { name })
}

fn secret(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<Vec<u8>, ConfigError> {
    let value = required(get, name)?;
    if value.len() < MIN_SECRET_BYTES {
        return Err(ConfigError::SecretTooShort {
            name,
            actual: value.len(),
        });
    }
    Ok(value.into_bytes())
}

fn path(get: &dyn Fn(&str) -> Option<String>, name: &str, default: &str) -> PathBuf {
    get(name)
        .filter(|v| !v.trim().is_empty())
        .map_or_else(|| PathBuf::from(default), PathBuf::from)
}

fn number<T>(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    match get(name).filter(|v| !v.trim().is_empty()) {
        None => Ok(default),
        Some(value) => value.trim().parse().map_err(|_| ConfigError::Invalid {
            name,
            value,
            expected: "number",
        }),
    }
}

fn parse<T>(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default: &str,
    expected: &'static str,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
{
    let value = get(name)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_owned());
    value.trim().parse().map_err(|_| ConfigError::Invalid {
        name,
        value,
        expected,
    })
}

/// Parse a duration written as a bare number of seconds or with a unit suffix.
///
/// `20`, `20s`, `15m`, `2h`, `7d` all work, because a deployment reads `2h` far more
/// easily than `7200` and a mistyped unit should not become a plausible wrong number.
fn duration(
    get: &dyn Fn(&str) -> Option<String>,
    name: &'static str,
    default_secs: u64,
) -> Result<Duration, ConfigError> {
    let Some(raw) = get(name).filter(|v| !v.trim().is_empty()) else {
        return Ok(Duration::from_secs(default_secs));
    };
    let value = raw.trim();
    let invalid = || ConfigError::Invalid {
        name,
        value: raw.clone(),
        expected: "duration such as 30s, 15m, 2h or 7d",
    };

    let (digits, multiplier) = match value.as_bytes().last() {
        Some(b's') => (&value[..value.len() - 1], 1),
        Some(b'm') => (&value[..value.len() - 1], 60),
        Some(b'h') => (&value[..value.len() - 1], 60 * 60),
        Some(b'd') => (&value[..value.len() - 1], 24 * 60 * 60),
        _ => (value, 1),
    };
    let amount: u64 = digits.parse().map_err(|_| invalid())?;
    amount
        .checked_mul(multiplier)
        .map(Duration::from_secs)
        .ok_or_else(invalid)
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(4))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const SALT: &str = "0123456789abcdef0123456789abcdef";

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = [
            ("PUBLIC_URL", "https://typst.example.com"),
            ("TENANT_SALT", SALT),
            ("SIGNING_SECRET", SALT),
            ("API_KEYS", "alice:sk_test"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        for (k, v) in pairs {
            if v.is_empty() {
                map.remove(*k);
            } else {
                map.insert((*k).to_owned(), (*v).to_owned());
            }
        }
        map
    }

    fn load(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map = env(pairs);
        Config::from_source(&move |name| map.get(name).cloned())
    }

    #[test]
    fn a_minimal_environment_loads_with_defaults() {
        let config = load(&[]).expect("loads");
        assert_eq!(config.bind_addr.port(), 3000);
        assert_eq!(config.compile_timeout, Duration::from_secs(20));
        assert_eq!(config.max_pages, 200);
        assert_eq!(config.preview_max_px, 2000);
        assert_eq!(config.limits.output_ttl, Duration::from_secs(2 * 60 * 60));
        assert!(config.oidc.is_none());
    }

    #[test]
    fn every_required_variable_is_named_when_missing() {
        for name in ["PUBLIC_URL", "TENANT_SALT", "SIGNING_SECRET"] {
            let err = load(&[(name, "")]).expect_err("must fail");
            assert_eq!(err, ConfigError::Missing { name });
            // The message has to name the full variable, or an operator has to guess.
            assert!(
                err.to_string().contains(&format!("TYPST_MCP_{name}")),
                "{err}"
            );
        }
    }

    #[test]
    fn short_secrets_are_refused() {
        // `changeme` would otherwise work perfectly until someone looked.
        for name in ["TENANT_SALT", "SIGNING_SECRET"] {
            let err = load(&[(name, "changeme")]).expect_err("must fail");
            assert_eq!(err, ConfigError::SecretTooShort { name, actual: 8 });
        }
    }

    #[test]
    fn starting_without_any_credentials_is_refused() {
        // The failure mode this prevents is an anonymous compute endpoint on the
        // public internet that looks like a successful deploy.
        let err = load(&[("API_KEYS", "")]).expect_err("must fail");
        assert_eq!(err, ConfigError::NoCredentials);
    }

    #[test]
    fn oidc_alone_is_enough() {
        let config = load(&[
            ("API_KEYS", ""),
            ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
            ("OIDC_AUDIENCE", "api://typst-mcp"),
        ])
        .expect("loads");
        assert!(config.api_keys.is_empty());
        let oidc = config.oidc.expect("configured");
        assert_eq!(oidc.audience, "api://typst-mcp");
        assert_eq!(oidc.scope, "render", "scope should default");
    }

    #[test]
    fn oidc_without_an_audience_is_refused() {
        // Accepting any audience would let a token minted for another application
        // authenticate here.
        let err = load(&[("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0")])
            .expect_err("must fail");
        assert_eq!(
            err,
            ConfigError::Missing {
                name: "OIDC_AUDIENCE"
            }
        );
    }

    #[test]
    fn durations_accept_units() {
        for (value, expected) in [
            ("30", 30),
            ("30s", 30),
            ("15m", 900),
            ("2h", 7200),
            ("7d", 604_800),
            ("  2h  ", 7200),
        ] {
            let config = load(&[("COMPILE_TIMEOUT", value)]).expect("loads");
            assert_eq!(
                config.compile_timeout,
                Duration::from_secs(expected),
                "{value:?}"
            );
        }
    }

    #[test]
    fn malformed_values_name_the_variable_and_what_was_expected() {
        for (name, value, expected) in [
            ("COMPILE_TIMEOUT", "soon", "duration"),
            ("COMPILE_TIMEOUT", "10x", "duration"),
            ("MAX_PAGES", "lots", "number"),
            ("BIND_ADDR", "not-an-address", "socket"),
        ] {
            let err = load(&[(name, value)]).expect_err("must fail");
            let message = err.to_string();
            assert!(message.contains(name), "{message}");
            assert!(message.contains(expected), "{message}");
        }
    }

    #[test]
    fn a_duration_that_would_overflow_is_refused() {
        let err = load(&[("COMPILE_TIMEOUT", "99999999999999999999d")]).expect_err("must fail");
        assert!(matches!(err, ConfigError::Invalid { .. }), "{err:?}");
    }

    #[test]
    fn urls_are_built_without_double_slashes() {
        let config = load(&[("PUBLIC_URL", "https://typst.example.com/")]).expect("loads");
        assert_eq!(
            config.url("files/x.pdf"),
            "https://typst.example.com/files/x.pdf"
        );
        assert_eq!(
            config.url("/files/x.pdf"),
            "https://typst.example.com/files/x.pdf"
        );
        assert_eq!(config.mcp_resource_url(), "https://typst.example.com/mcp");
        assert_eq!(
            config.metadata_url(),
            "https://typst.example.com/.well-known/oauth-protected-resource/mcp"
        );
    }

    #[test]
    fn font_directories_are_colon_separated() {
        let config = load(&[("FONT_DIRS", "/a:/b:/c")]).expect("loads");
        assert_eq!(config.font_dirs.len(), 3);
        assert_eq!(config.font_dirs[2], PathBuf::from("/c"));
    }

    #[test]
    fn entra_oauth_urls_come_from_the_issuer() {
        let config = load(&[
            ("OIDC_ISSUER", "https://login.microsoftonline.com/abc/v2.0"),
            ("OIDC_AUDIENCE", "api://typst-mcp"),
            ("OIDC_CLIENT_ID", "entra-public-client"),
        ])
        .expect("loads");
        assert_eq!(config.client_id.as_deref(), Some("entra-public-client"));
        assert_eq!(config.dcr_client_id.as_deref(), Some("entra-public-client"));
        assert_eq!(
            config.entra_authorize_url().as_deref(),
            Some("https://login.microsoftonline.com/abc/oauth2/v2.0/authorize")
        );
        assert_eq!(
            config.entra_token_url().as_deref(),
            Some("https://login.microsoftonline.com/abc/oauth2/v2.0/token")
        );
        assert!(
            config
                .oauth_redirect_uris
                .iter()
                .any(|uri| uri == "cursor://anysphere.cursor-mcp/oauth/callback")
        );
        assert!(
            config
                .oauth_redirect_uris
                .iter()
                .any(|uri| uri == "claude://claude.ai/oauth/callback")
        );
    }
}
