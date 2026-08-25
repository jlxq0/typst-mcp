//! Shared redirect URI validation for the OAuth proxy and DCR shim.

use anyhow::{Context, Result};
use url::Url;

/// Comma-separated exact redirect URI allowlist for proxied OAuth clients.
pub const ENV_OAUTH_REDIRECT_URIS: &str = "TYPST_MCP_OAUTH_REDIRECT_URIS";

pub fn parse_allowlist(raw: &str, key: &str) -> Result<Vec<String>> {
    let mut uris = Vec::new();
    for uri in raw.split(',').map(str::trim).filter(|uri| !uri.is_empty()) {
        validate_redirect_uri(uri, key)?;
        if !uris.iter().any(|allowed| allowed == uri) {
            uris.push(uri.to_owned());
        }
    }
    if uris.is_empty() {
        anyhow::bail!("{key} must contain at least one redirect URI");
    }
    Ok(uris)
}

pub fn is_allowed_redirect_uri(allowed: &[String], uri: &str) -> bool {
    if validate_redirect_uri(uri, "redirect_uri").is_err() {
        return false;
    }
    if allowed.iter().any(|allowed| allowed == uri) {
        return true;
    }
    // RFC 8252 §7.3: a native app binds an ephemeral loopback port, so the
    // authorization server must allow any port for a loopback redirect. Only
    // the port is relaxed, and only for cleartext loopback entries: scheme,
    // host, path and query must still match exactly, so `/callback` never
    // matches `/oauth/callback` and `localhost` never matches `127.0.0.1`.
    let Some(requested) = parse_loopback_http(uri) else {
        return false;
    };
    allowed
        .iter()
        .filter_map(|allowed| parse_loopback_http(allowed))
        .any(|entry| loopback_matches_ignoring_port(&entry, &requested))
}

/// `Some(url)` only for a cleartext `http` URL on a loopback host — the one
/// case RFC 8252 §7.3 lets the port vary.
///
/// The scheme check is load-bearing on the *requested* side: without it
/// `https://localhost:3118/callback` would match an `http` loopback entry.
/// The `is_loopback_host` check is redundant today — `validate_redirect_uri`
/// already rejects cleartext non-loopback hosts on both the request and the
/// allowlist entry, and host equality would refuse the pair anyway — so no
/// test can kill it by mutation. It stays as the second line of defence for a
/// caller that hands `is_allowed_redirect_uri` a list which never went through
/// `parse_allowlist`.
fn parse_loopback_http(uri: &str) -> Option<Url> {
    let url = Url::parse(uri).ok()?;
    if url.scheme() != "http" {
        return None;
    }
    if !is_loopback_host(url.host_str()?) {
        return None;
    }
    Some(url)
}

fn loopback_matches_ignoring_port(entry: &Url, requested: &Url) -> bool {
    entry.host_str() == requested.host_str()
        && entry.path() == requested.path()
        && entry.query() == requested.query()
}

/// Loopback hosts accepted for cleartext `http://` redirect URIs
/// (RFC 8252 §7.3). Anything else over `http` would put the authorization
/// code on the wire in cleartext.
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

fn validate_redirect_uri(uri: &str, key: &str) -> Result<()> {
    if uri.trim() != uri || uri.is_empty() {
        anyhow::bail!(
            "{key} entries must be non-empty absolute URLs without surrounding whitespace"
        );
    }
    let url = Url::parse(uri).with_context(|| format!("invalid {key} redirect URI: {uri}"))?;
    match url.scheme() {
        "https" => {
            if url.host_str().is_none() {
                anyhow::bail!("{key} https entries must include a host: {uri}");
            }
        }
        // RFC 8252 §7.3 loopback interface redirection. Native apps bind an
        // ephemeral local port, so this is the one case where cleartext is
        // acceptable — but only on a loopback host.
        "http" => {
            let host = url.host_str().unwrap_or_default();
            if !is_loopback_host(host) {
                anyhow::bail!(
                    "{key} http entries are only allowed on loopback hosts \
                     (localhost, 127.0.0.1, [::1]): {uri}"
                );
            }
        }
        // RFC 8252 §7.1 private-use ("custom") URI schemes, e.g.
        // `cursor://…` / `grokbot://…` used by native MCP clients. The exact
        // string allowlist in `is_allowed_redirect_uri` is the actual control
        // — an operator must list the URI explicitly — so this arm only
        // rejects structurally broken input.
        scheme => {
            if scheme.is_empty() {
                anyhow::bail!("{key} entries must have a scheme: {uri}");
            }
        }
    }
    if url.fragment().is_some() {
        anyhow::bail!("{key} entries must not contain URI fragments: {uri}");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("{key} entries must not contain user info: {uri}");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_matches_exact_redirect_uri_only() {
        let allowed = parse_allowlist("https://claude.ai/api/mcp/auth_callback", "TEST").unwrap();

        assert!(is_allowed_redirect_uri(
            &allowed,
            "https://claude.ai/api/mcp/auth_callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://claude.ai/api/mcp/auth_callback/"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://attacker.example/callback"
        ));
    }

    #[test]
    fn allowlist_rejects_fragments_and_userinfo() {
        assert!(parse_allowlist("https://claude.ai/cb#frag", "TEST").is_err());
        assert!(parse_allowlist("https://user@claude.ai/cb", "TEST").is_err());
    }

    /// RFC 8252 §7.1 — native MCP clients (Cursor / Grok Bot desktop) register
    /// private-use scheme callbacks. They must survive `parse_allowlist` (which
    /// runs at startup over the env var) and then match exactly.
    #[test]
    fn allowlist_accepts_private_use_schemes() {
        let allowed = parse_allowlist(
            "cursor://anysphere.cursor-mcp/oauth/callback,grokbot://mcp/oauth/callback",
            "TEST",
        )
        .unwrap();

        assert!(is_allowed_redirect_uri(
            &allowed,
            "cursor://anysphere.cursor-mcp/oauth/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "grokbot://mcp/oauth/callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "cursor://anysphere.cursor-mcp/oauth/callback/extra"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "evil://mcp/oauth/callback"
        ));
    }

    /// RFC 8252 §7.3 — loopback HTTP is allowed; any other cleartext host is
    /// not. Never `allow_insecure_uris`.
    #[test]
    fn http_is_loopback_only() {
        for uri in [
            "http://localhost:8787/callback",
            "http://127.0.0.1:8787/callback",
        ] {
            assert!(parse_allowlist(uri, "TEST").is_ok(), "should accept {uri}");
        }
        for uri in [
            "http://evil.example/callback",
            "http://localhost.evil.example/callback",
        ] {
            assert!(parse_allowlist(uri, "TEST").is_err(), "should reject {uri}");
        }
    }

    /// RFC 8252 §7.3 — a native app binds an ephemeral loopback port, so an
    /// allowlisted loopback entry must match whatever port the client drew.
    /// Claude Code CLI picks a random free port per session; the observed
    /// failing attempt used 3118 against an allowlist carrying only 8787.
    #[test]
    fn loopback_entry_matches_any_port() {
        let allowed = parse_allowlist("http://localhost:8787/callback", "TEST").unwrap();

        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:8787/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost/callback"
        ));
    }

    /// Only the port is relaxed. Path, host and query stay exact.
    #[test]
    fn loopback_port_relaxation_keeps_host_path_and_query_exact() {
        let allowed = parse_allowlist("http://localhost:8787/callback", "TEST").unwrap();

        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/other"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback/extra"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback?code=stolen"
        ));
        // RFC 8252 relaxes the port, not the host: 127.0.0.1 must be listed
        // separately to be accepted.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://127.0.0.1:3118/callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://[::1]:3118/callback"
        ));
        // Still not a redirect target we would ever reach: non-loopback
        // cleartext fails validation before matching.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://evil.example:8787/callback"
        ));
    }

    /// Two loopback entries differing only by path must stay distinct — a
    /// port-agnostic comparison must not degrade into a prefix match.
    ///
    /// This is not a synthetic shape. The running deployment carries both
    /// `http://localhost:8787/callback` and `http://localhost:8787/oauth/callback`
    /// — same scheme, same host, same port, differing only by path (read off
    /// the live container env 2026-08-25). A path relaxation would silently
    /// merge those two in production, so keep the pair here and do not weaken
    /// path comparison to a prefix or a `starts_with`.
    #[test]
    fn loopback_paths_stay_distinct() {
        let allowed = parse_allowlist(
            "http://localhost:8787/callback,http://localhost:8787/oauth/callback",
            "TEST",
        )
        .unwrap();

        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/oauth/callback"
        ));
        // Neither entry may absorb a third path, in either direction.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback/oauth"
        ));
        assert!(!is_allowed_redirect_uri(&allowed, "http://localhost:3118/"));
        // ...and a different loopback host is still a different entry.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://127.0.0.1:3118/callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://127.0.0.1:3118/oauth/callback"
        ));
    }

    /// The scheme is guarded on both sides, and each side has its own
    /// assertion here because each fails differently.
    ///
    /// Drop the check on the *requested* URI and `https://localhost:3118/callback`
    /// matches an `http` loopback entry: TLS on loopback is not the case
    /// RFC 8252 §7.3 carves out, and the mismatch means the caller is not the
    /// client that registered.
    ///
    /// Drop it on the *allowlist entry* and an `https://localhost:8443/callback`
    /// entry port-relaxes into a cleartext `http://localhost:3118/callback` —
    /// an https-to-http downgrade on an entry an operator wrote expecting TLS,
    /// putting the authorization code on the wire in the clear. That is worse
    /// than the lockout this commit fixes, so it gets its own assertion rather
    /// than riding on the first one.
    #[test]
    fn loopback_relaxation_does_not_cross_schemes() {
        let allowed = parse_allowlist("http://localhost:8787/callback", "TEST").unwrap();
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://localhost:3118/callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://localhost:8787/callback"
        ));

        // ...and the mirror: an `https` loopback entry is matched exactly, so
        // it never picks up a cleartext request or a different port.
        let https_entry = parse_allowlist("https://localhost:8787/callback", "TEST").unwrap();
        assert!(!is_allowed_redirect_uri(
            &https_entry,
            "http://localhost:8787/callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &https_entry,
            "https://localhost:3118/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &https_entry,
            "https://localhost:8787/callback"
        ));
    }

    /// `url` canonicalises the host before anything here sees it: `127.1`,
    /// `0177.0.0.1` and `2130706433` all parse to `127.0.0.1`. Two consequences
    /// worth pinning, because neither is visible in the matcher's own text.
    ///
    /// Host spelling cannot be used to slip past the loopback carve-out, and it
    /// cannot be used to evade an allowlist entry either — comparison is on the
    /// canonical form, so an entry written `127.0.0.1` matches a request
    /// written `127.1`. Those are the same host, so that is the right answer.
    ///
    /// It also means the host term enforces nothing against an obfuscated
    /// downgrade attempt: the entry-side scheme guard is the whole control.
    #[test]
    fn obfuscated_loopback_spellings_canonicalise() {
        let allowed = parse_allowlist("http://127.0.0.1:8787/callback", "TEST").unwrap();
        for spelling in [
            "http://127.1:3118/callback",
            "http://0177.0.0.1:3118/callback",
            "http://2130706433:3118/callback",
            "http://127.0.0.1:3118/callback",
        ] {
            assert!(
                is_allowed_redirect_uri(&allowed, spelling),
                "{spelling} is 127.0.0.1 and must match"
            );
        }
        // ...and `localhost` is still a different host from every one of them.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback"
        ));

        // The downgrade is refused for every spelling, by the scheme guard
        // alone — each of these is a loopback host by the time it is checked.
        let https_entry = parse_allowlist("https://127.0.0.1:8443/callback", "TEST").unwrap();
        for spelling in [
            "http://127.0.0.1:3118/callback",
            "http://127.1:3118/callback",
            "http://0177.0.0.1:3118/callback",
        ] {
            assert!(
                !is_allowed_redirect_uri(&https_entry, spelling),
                "{spelling} must not downgrade an https entry to cleartext"
            );
        }
    }

    /// The port is meaningful for anything that is not cleartext loopback.
    /// Relaxing it on `https://claude.ai/...` would be a real hole.
    #[test]
    fn non_loopback_entries_keep_exact_port_matching() {
        let allowed = parse_allowlist(
            "https://claude.ai/api/mcp/auth_callback,\
             cursor://anysphere.cursor-mcp/oauth/callback",
            "TEST",
        )
        .unwrap();

        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "cursor://anysphere.cursor-mcp:8443/oauth/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "https://claude.ai/api/mcp/auth_callback"
        ));
    }

    /// A hand-copied snapshot of the allowlist the deployment was running on
    /// 2026-08-25, read off the live container env rather than off a manifest.
    ///
    /// **This test cannot detect drift, and must not be read as if it could.**
    /// `raw` is a literal and the assertions count what that literal parsed to,
    /// so the deployment is not an input and no change to it can turn this red.
    /// It pins matcher *behaviour* against a shape production really had; the
    /// only way to know the shape is still current is to ask the thing
    /// enforcing it:
    ///
    /// ```text
    /// kubectl -n typst-mcp get deploy \
    ///   -o jsonpath='{range .items[*].spec.template.spec.containers[*].env[*]}{.name}={.value}{"\n"}{end}' \
    ///   | grep REDIRECT
    /// ```
    ///
    /// The previous version of this test claimed to be "the exact set the
    /// deployment ships" while carrying nine entries against the live seven,
    /// and stayed green throughout — which is the failure mode being named
    /// here, not a fixed bug.
    ///
    /// Two of the seven are loopback URIs sharing a host and port and differing
    /// only by path, which is why `loopback_paths_stay_distinct` guards a real
    /// production shape rather than an invented one.
    #[test]
    fn production_allowlist_snapshot_behaves() {
        let raw = "https://claude.ai/api/mcp/auth_callback,\
                   https://claude.com/api/mcp/auth_callback,\
                   https://www.cursor.com/agents/mcp/oauth/callback,\
                   cursor://anysphere.cursor-mcp/oauth/callback,\
                   grokbot://mcp/oauth/callback,\
                   http://localhost:8787/callback,\
                   http://localhost:8787/oauth/callback";
        let allowed = parse_allowlist(raw, ENV_OAUTH_REDIRECT_URIS).unwrap();
        assert_eq!(allowed.len(), 7);

        // Claude Code CLI draws a random loopback port; both deployed loopback
        // paths must accept it, and must not accept each other's path.
        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/callback"
        ));
        assert!(is_allowed_redirect_uri(
            &allowed,
            "http://localhost:3118/oauth/callback"
        ));
        // Everything non-loopback in the deployed set still needs its exact port.
        assert!(!is_allowed_redirect_uri(
            &allowed,
            "https://claude.ai:8443/api/mcp/auth_callback"
        ));
    }

    /// Shapes the allowlist must keep parsing, whether or not the deployment
    /// currently lists them — private-use schemes registered by native MCP
    /// clients. Makes no claim about production; see
    /// `production_allowlist_snapshot_behaves` for the snapshot, and note that
    /// it cannot detect drift either.
    #[test]
    fn allowlist_accepts_the_shapes_we_support() {
        let raw = "claude://claude.ai/oauth/callback,\
                   claude://oauth/callback,\
                   cowork://oauth/callback";
        let allowed = parse_allowlist(raw, ENV_OAUTH_REDIRECT_URIS).unwrap();
        assert_eq!(allowed.len(), 3);
        assert!(is_allowed_redirect_uri(&allowed, "claude://oauth/callback"));
    }
}
