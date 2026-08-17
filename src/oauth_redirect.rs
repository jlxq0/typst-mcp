//! Exact OAuth redirect-URI allowlisting with native-app custom schemes.
//!
//! Custom schemes are first-class (RFC 8252 §7.1). Loopback http is allowed
//! only on localhost / 127.0.0.1 / ::1 (RFC 8252 §7.3). Never `allow_insecure_uris`.

use anyhow::{Context, Result};
use url::Url;

pub const ENV_OAUTH_REDIRECT_URIS: &str = "TYPST_MCP_OAUTH_REDIRECT_URIS";

pub const DEFAULT_REDIRECT_URIS: &[&str] = &[
    "cursor://anysphere.cursor-mcp/oauth/callback",
    "grokbot://mcp/oauth/callback",
    "http://localhost:8787/callback",
    "https://www.cursor.com/agents/mcp/oauth/callback",
    "https://claude.ai/api/mcp/auth_callback",
    "https://claude.com/api/mcp/auth_callback",
    "claude://claude.ai/oauth/callback",
    "claude://oauth/callback",
    "cowork://oauth/callback",
];

const BLOCKED_SCHEMES: &[&str] = &["javascript", "data", "file", "vbscript", "blob", "about"];
const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];

pub fn parse_allowlist(raw: &str, key: &str) -> Result<Vec<String>> {
    let mut uris = Vec::new();
    for part in raw.split(',') {
        let uri = part.trim();
        if uri.is_empty() {
            continue;
        }
        validate_redirect_uri(uri, key)?;
        if !uris.iter().any(|existing| existing == uri) {
            uris.push(uri.to_owned());
        }
    }
    if uris.is_empty() {
        anyhow::bail!("{key} must contain at least one redirect URI");
    }
    Ok(uris)
}

pub fn merge_allowlist(extra: &[String]) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for uri in DEFAULT_REDIRECT_URIS
        .iter()
        .copied()
        .chain(extra.iter().map(String::as_str))
    {
        validate_redirect_uri(uri, "redirect_uri")?;
        if !result.iter().any(|existing| existing == uri) {
            result.push(uri.to_owned());
        }
    }
    Ok(result)
}

pub fn is_allowed_redirect_uri(allowed: &[String], uri: &str) -> bool {
    validate_redirect_uri(uri, "redirect_uri").is_ok()
        && allowed.iter().any(|candidate| candidate == uri)
}

pub fn validate_redirect_uri(uri: &str, key: &str) -> Result<Url> {
    if uri.is_empty() || uri.trim() != uri {
        anyhow::bail!(
            "{key} entries must be non-empty absolute URLs without surrounding whitespace"
        );
    }
    let url = Url::parse(uri).with_context(|| format!("invalid {key} redirect URI: {uri}"))?;
    let scheme = url.scheme().to_ascii_lowercase();
    if BLOCKED_SCHEMES.contains(&scheme.as_str()) {
        anyhow::bail!("{key} entries must not use dangerous schemes: {uri}");
    }
    if url.fragment().is_some() {
        anyhow::bail!("{key} entries must not contain URI fragments: {uri}");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("{key} entries must not contain user info: {uri}");
    }

    match scheme.as_str() {
        "http" => {
            let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
            if !LOOPBACK_HOSTS.contains(&host.as_str()) {
                anyhow::bail!(
                    "{key} http entries are only allowed on loopback hosts \
                     (localhost, 127.0.0.1, [::1]): {uri}"
                );
            }
        }
        "https" => {
            if url.host_str().is_none() {
                anyhow::bail!("{key} https entries must include a host: {uri}");
            }
        }
        scheme if scheme.is_empty() => {
            anyhow::bail!("{key} entries must have a scheme: {uri}");
        }
        _ => {}
    }
    Ok(url)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_and_match() {
        let allowed = merge_allowlist(&[]).unwrap();
        for uri in DEFAULT_REDIRECT_URIS {
            assert!(is_allowed_redirect_uri(&allowed, uri), "{uri}");
        }
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://attacker.example/callback"
        ));
        assert!(!is_allowed_redirect_uri(&allowed, "evil://mcp/oauth/callback"));
    }

    #[test]
    fn http_is_loopback_only() {
        assert!(parse_allowlist("http://localhost:8787/callback", "TEST").is_ok());
        assert!(parse_allowlist("http://127.0.0.1:8787/callback", "TEST").is_ok());
        assert!(parse_allowlist("http://evil.example/callback", "TEST").is_err());
    }

    #[test]
    fn rejects_fragments_and_userinfo() {
        assert!(parse_allowlist("https://claude.ai/cb#frag", "TEST").is_err());
        assert!(parse_allowlist("https://user@claude.ai/cb", "TEST").is_err());
    }
}
