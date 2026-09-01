# typst-mcp

A remote MCP server and REST API that renders branded PDFs with [Typst](https://typst.app).
Streamable HTTP at `/mcp`, behind Microsoft Entra OIDC. Compiles run in a
short-lived subprocess so Typst's process-global interners cannot accumulate
across documents.

Self-host the `/mcp` endpoint on your own domain.

## Status

Production runs `v0.2.0` at `https://typst-mcp.hanso.group`. It ships Hanso, KSC, Lenno
and Freudenberg templates and the complete eight-tool MCP surface. The deployed image is
the exact linux/amd64 artifact that passed the ten-step REST/MCP smoke suite; the release
also passed the 10,000-distinct-filename soak, dependency policy checks, and security
review. Current distribution proof is recorded in
[`docs/release-evidence.md`](docs/release-evidence.md).

## Shape (RFC 9728 / MCP authorization)

Matches [matrix-mcp](https://forge.oddie.app/jlxq0/matrix-mcp), not the older
origin-only metadata:

| Piece | Value |
|---|---|
| MCP endpoint | `{origin}/mcp` |
| `resource` (RFC 9728) | `{origin}/mcp` |
| Metadata | `/.well-known/oauth-protected-resource/mcp` |
| Origin probe | `/.well-known/oauth-protected-resource` (same document) |
| 401 on `/mcp` | `WWW-Authenticate: Bearer resource_metadata="{origin}/.well-known/oauth-protected-resource/mcp"` |
| `authorization_servers` | `{origin}` — this process, not Entra directly |
| AS metadata (RFC 8414) | `/.well-known/oauth-authorization-server`, also served at `…/mcp`, `/.well-known/openid-configuration` and `…/mcp` |
| DCR (RFC 7591) | `POST /register` |
| Proxied to Entra | `GET /authorize`, `POST /token`, `GET /oauth/callback` |

The server validates Entra access tokens. Claude Desktop/Cowork insist on
Dynamic Client Registration, so this process also fronts Entra: it serves
RFC 8414 metadata with a `registration_endpoint`, a `/register` shim that
hands out a pre-provisioned public SPA client, and same-origin `/authorize`
+ `/token` that proxy to Hanso Entra. Entra only ever sees
`{origin}/oauth/callback`.

That indirection is what makes native clients work at all. Grok Bot and Cursor
call back to private-use schemes (`grokbot://…`, `cursor://…`), which a web
app registration cannot hold; the proxy stores the client's redirect URI,
hands Entra its own HTTPS callback, and restores the scheme on the way back.
Only URIs listed in `TYPST_MCP_OAUTH_REDIRECT_URIS` are accepted, at
`/register`, `/authorize` and `/token` alike — an exact-string allowlist, and
cleartext `http` only on a loopback host (RFC 8252 §7.3). There is no
"allow insecure URIs" switch and there will not be one.

## Tools

| Tool | What it does |
|---|---|
| `typst_render` | Render a named brand template to PDF + preview |
| `typst_compile` | Compile an uploaded Typst bundle |
| `typst_templates` | List shipped templates |
| `typst_template_schema` | JSON Schema for a template's arguments |
| `typst_fonts` | Fonts available to compiles |
| `typst_assets` | List uploaded assets for the caller |
| `typst_link` | Mint a short-lived signed download URL |
| `typst_upload_template` | Create a tenant-scoped text-only template |

REST (`/api/v1`) is the same surface behind static API keys, for services that
should not put a long-lived secret in a desktop MCP client.

## Environment

Every variable is prefixed `TYPST_MCP_`. The process refuses to start if a
required secret is missing or shorter than 32 bytes, or if neither API keys nor
OIDC is configured.

### Required

| Variable | Meaning |
|---|---|
| `TYPST_MCP_PUBLIC_URL` | Public origin, no trailing slash. Example: `https://typst-mcp.your-domain.example` |
| `TYPST_MCP_TENANT_SALT` | ≥32 bytes. Derives per-caller storage partitions. Rotating it re-partitions every tenant. |
| `TYPST_MCP_SIGNING_SECRET` | ≥32 bytes. Keys signed download URLs. Rotating it invalidates outstanding links. |

### Credentials (at least one door)

| Variable | Meaning |
|---|---|
| `TYPST_MCP_OIDC_ISSUER` | Entra issuer, e.g. `https://login.microsoftonline.com/<tenant-id>/v2.0`. Do not invent the tenant GUID — copy it from Entra. |
| `TYPST_MCP_OIDC_AUDIENCE` | Comma-separated `aud` values this server accepts. Required when issuer is set. **Put the App ID URI first** (it also qualifies bare scopes for the authorize request) and the API's **client-ID GUID** second — Entra puts the GUID in every v2.0 access token, so a URI-only setting rejects every real token. |
| `TYPST_MCP_OIDC_TENANT_ID` | Optional directory GUID, checked against the token `tid`. |
| `TYPST_MCP_OIDC_SCOPE` | Scope a token must carry. Default: `render`. |
| `TYPST_MCP_DCR_CLIENT_ID` | Pre-provisioned Entra public SPA `client_id` returned by `/register`. |
| `TYPST_MCP_OAUTH_REDIRECT_URIS` | Comma-separated exact redirect URIs the DCR shim and OAuth proxy accept. Required when DCR is set. Custom schemes are first-class; `http` is loopback-only. |
| `TYPST_MCP_API_KEYS` | `name:secret,name:secret` for `/api/v1`; each secret must be at least 32 bytes and labels must be unique. Not accepted on `/mcp`. |

### Common optional

| Variable | Default | Meaning |
|---|---|---|
| `TYPST_MCP_BIND_ADDR` | `0.0.0.0:3000` | Listen address |
| `TYPST_MCP_DATA_DIR` | `/data` | Tenant-partitioned store |
| `TYPST_MCP_TEMPLATE_DIR` | `/usr/share/typst-mcp/templates` | Shipped templates |
| `TYPST_MCP_FONT_DIRS` | `/usr/share/fonts/typst` | Colon-separated font dirs |
| `TYPST_MCP_LOG_FORMAT` | `json` | `json` or pretty |
| `TYPST_MCP_COMPILE_TIMEOUT` | `20s` | Per-compile deadline (`20`, `20s`, `15m`, `2h`, `7d`) |
| `TYPST_MCP_METRICS_BIND_ADDR` | `0.0.0.0:9090` | Separate listener exposing only `GET /metrics` |

Set the standard OpenTelemetry variables `OTEL_EXPORTER_OTLP_ENDPOINT` and, optionally,
`OTEL_SERVICE_NAME` to export traces over OTLP/gRPC. With no endpoint, no exporter or
telemetry connection is created. Metrics and audit events contain bounded operational
labels and counts only; source, input data, rendered content, diagnostics, and credentials
are excluded by their APIs and regression tests.

## Compile-process safety

Typst permanently interns every distinct `FileId` in a 16-bit process-global table, while
`comemo` also retains a process-global memo cache. Neither is safely reclaimable in a
long-lived renderer. This server therefore starts one credential-free subprocess for each
compile and discards the entire process afterward. Do not replace that boundary with
in-process compilation or `FileId::unique()`. The design, limits, and threat controls are
recorded in [`docs/security.md`](docs/security.md); distribution proof is recorded in
[`docs/release-evidence.md`](docs/release-evidence.md).

## Entra app (click-path)

typst-mcp already speaks Entra OIDC. Create the app in **this org only**; do not
invent tenant or client GUIDs — copy them from the portal after the app exists.

1. Entra admin center → **Identity** → **Applications** → **App registrations** → **New registration**.
2. Name: `typst-mcp`.
3. Supported account types: **Accounts in this organizational directory only**.
4. Redirect URI: **none on this API registration**. The redirect URIs belong on the
   second, public-client registration below.
5. **Expose an API** → set the Application ID URI. That value is the **first** entry
   of `TYPST_MCP_OIDC_AUDIENCE`.
6. Add a scope named `render` (the code default).
6b. Overview → copy the **Application (client) ID** of this API registration and
   append it to `TYPST_MCP_OIDC_AUDIENCE` after the URI. This is not optional
   housekeeping: with `requestedAccessTokenVersion: 2`, every access token Entra
   mints carries the **GUID** in `aud`, so a server configured with the URI alone
   401s every request and clients loop through re-auth forever.
7. Overview → copy **Directory (tenant) ID**. Then:
   - `TYPST_MCP_OIDC_ISSUER=https://login.microsoftonline.com/<tenant-id>/v2.0`
   - optionally `TYPST_MCP_OIDC_TENANT_ID=<tenant-id>` so a token from another
     directory cannot authenticate here.
8. Create a second public PKCE client, pre-authorise it for the API scope, and register
   only `https://typst-mcp.hanso.group/oauth/callback` with Entra.
9. Set that public client's id as `TYPST_MCP_DCR_CLIENT_ID` and configure the exact client
   callbacks accepted by the bridge in `TYPST_MCP_OAUTH_REDIRECT_URIS`. Claude/Cursor/Grok
   callbacks terminate at typst-mcp and are never added to Entra directly.

Store issuer, audience, tenant salt, and signing secret in your secret manager.
The image pull secret is whatever your registry requires.

## Local development

Rust 1.93+ with `edition = "2024"`.

```sh
export TYPST_MCP_PUBLIC_URL=http://127.0.0.1:3000
export TYPST_MCP_TENANT_SALT=0123456789abcdef0123456789abcdef
export TYPST_MCP_SIGNING_SECRET=0123456789abcdef0123456789abcdef
export TYPST_MCP_API_KEYS=dev:sk_dev_0123456789abcdef0123456789abcdef
export TYPST_MCP_TEMPLATE_DIR=./templates
export TYPST_MCP_FONT_DIRS=./fonts
export TYPST_MCP_DATA_DIR=./data
export TYPST_MCP_LOG_FORMAT=pretty
cargo run
```

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
```

OIDC is optional locally. Production typically uses Entra only for `/mcp`.

## Image / CI

Forgejo CI (`.forgejo/workflows/ci.yml`) runs fmt, clippy, test, audit and deny, then builds
a local linux/amd64 image tarball on every ref, loads it, and runs `scripts/smoke.sh` against
that exact image. Only a `v*` tag can add registry tags and push the already-smoked image to
`forge.oddie.app/jlxq0/typst-mcp`; no post-smoke rebuild occurs. A nightly scheduled run
also executes the 10,000-distinct-filename RSS soak.

Production deployment is GitOps-managed from
`oddie-apps/platform/clusters/fondue/typst-mcp/`. A release tag publishes the already
smoked image to Forge; a reviewed platform change pins its digest, and Argo CD reconciles
the deployment, service, PVC, ExternalSecret, HTTPRoute, network policy, metrics scrape,
and alert rule. Forge is the canonical git and image origin.

```sh
docker run --rm -p 3000:3000 \
  -e TYPST_MCP_PUBLIC_URL=https://typst-mcp.hanso.group \
  -e TYPST_MCP_TENANT_SALT=... \
  -e TYPST_MCP_SIGNING_SECRET=... \
  -e TYPST_MCP_OIDC_ISSUER=https://login.microsoftonline.com/<tenant-id>/v2.0 \
  -e TYPST_MCP_OIDC_AUDIENCE=api://typst-mcp,<api-app-guid> \
  -v typst-data:/data \
  forge.oddie.app/jlxq0/typst-mcp:v0.2.0
```

The live service is a single replica with a 5 GiB ReadWriteOnce PVC and Recreate rollout
strategy. TTL and per-tenant/global LRU quotas govern content; the PVC survives ordinary
pod replacements, but rendered output remains a re-creatable cache rather than durable
business data. Deployment secrets come from `typst-mcp-www` in the `Oddie Apps` vault.

## Licence

[MIT](LICENSE).
