# typst-mcp

A remote MCP server and REST API that renders branded PDFs with [Typst](https://typst.app).
Streamable HTTP at `/mcp`, behind Microsoft Entra OIDC. Compiles run in a
short-lived subprocess so Typst's process-global interners cannot accumulate
across documents.

Live URL (once DNS + Entra + Argo are in place):
`https://typst-mcp.kampong.social/mcp`

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

The server is a **resource server**. It validates Entra access tokens; it does
not run an OAuth authorization-code proxy and has no `/oauth/callback`.

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

REST (`/api/v1`) is the same surface behind static API keys, for services that
should not put a long-lived secret in a desktop MCP client.

## Environment

Every variable is prefixed `TYPST_MCP_`. The process refuses to start if a
required secret is missing or shorter than 32 bytes, or if neither API keys nor
OIDC is configured.

### Required

| Variable | Meaning |
|---|---|
| `TYPST_MCP_PUBLIC_URL` | Public origin, no trailing slash. Production: `https://typst-mcp.kampong.social` |
| `TYPST_MCP_TENANT_SALT` | ≥32 bytes. Derives per-caller storage partitions. Rotating it re-partitions every tenant. |
| `TYPST_MCP_SIGNING_SECRET` | ≥32 bytes. Keys signed download URLs. Rotating it invalidates outstanding links. |

### Credentials (at least one door)

| Variable | Meaning |
|---|---|
| `TYPST_MCP_OIDC_ISSUER` | Entra issuer, e.g. `https://login.microsoftonline.com/<tenant-id>/v2.0`. Do not invent the tenant GUID — copy it from Entra. |
| `TYPST_MCP_OIDC_AUDIENCE` | App ID URI or client id this server accepts as `aud`. Required when issuer is set. |
| `TYPST_MCP_OIDC_TENANT_ID` | Optional directory GUID, checked against the token `tid`. |
| `TYPST_MCP_OIDC_SCOPE` | Scope a token must carry. Default: `render`. |
| `TYPST_MCP_API_KEYS` | `name:secret,name:secret` for `/api/v1`. Not accepted on `/mcp`. |

### Common optional

| Variable | Default | Meaning |
|---|---|---|
| `TYPST_MCP_BIND_ADDR` | `0.0.0.0:3000` | Listen address |
| `TYPST_MCP_DATA_DIR` | `/data` | Tenant-partitioned store |
| `TYPST_MCP_TEMPLATE_DIR` | `/usr/share/typst-mcp/templates` | Shipped templates |
| `TYPST_MCP_FONT_DIRS` | `/usr/share/fonts/typst` | Colon-separated font dirs |
| `TYPST_MCP_LOG_FORMAT` | `json` | `json` or pretty |
| `TYPST_MCP_COMPILE_TIMEOUT` | `20s` | Per-compile deadline (`20`, `20s`, `15m`, `2h`, `7d`) |

## Entra app (click-path)

typst-mcp already speaks Entra OIDC. Create the app in **this org only**; do not
invent tenant or client GUIDs — copy them from the portal after the app exists.

1. Entra admin center → **Identity** → **Applications** → **App registrations** → **New registration**.
2. Name: `typst-mcp`.
3. Supported account types: **Accounts in this organizational directory only**.
4. Redirect URI: **none on this registration**. This process is a resource server
   and has no `/oauth/callback`. (jmap-mcp's `https://…/oauth/callback` is a
   different shape — an OAuth proxy. Do not add that path here.)
5. **Expose an API** → set the Application ID URI. That value is
   `TYPST_MCP_OIDC_AUDIENCE`.
6. Add a scope named `render` (the code default).
7. Overview → copy **Directory (tenant) ID**. Then:
   - `TYPST_MCP_OIDC_ISSUER=https://login.microsoftonline.com/<tenant-id>/v2.0`
   - optionally `TYPST_MCP_OIDC_TENANT_ID=<tenant-id>` so a token from another
     directory cannot authenticate here.
8. MCP *clients* (Claude, Cursor, …) need their own Entra registration — or to
   be authorized on this API — with **their** redirect URIs
   (`https://claude.ai/api/mcp/auth_callback`, etc.). Do not invent those client
   IDs; add them when the first client is wired.

Store issuer, audience, tenant salt, and signing secret in a 1Password item
`typst-mcp-www` in the **Gruyere** vault. The cluster pull secret is shared from
the existing `matrix-mcp-www` item (`forge-dockerconfigjson`).

## Local development

Rust 1.93+ with `edition = "2024"`.

```sh
export TYPST_MCP_PUBLIC_URL=http://127.0.0.1:3000
export TYPST_MCP_TENANT_SALT=0123456789abcdef0123456789abcdef
export TYPST_MCP_SIGNING_SECRET=0123456789abcdef0123456789abcdef
export TYPST_MCP_API_KEYS=dev:sk_dev_0123456789abcdef
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

OIDC is optional locally. Production on fondue uses Entra only for `/mcp`.

## Image / CI

Forgejo CI (`.forgejo/workflows/ci.yml`) matches matrix-mcp: fmt, clippy, test,
audit, deny, then `buildctl` against Pada's buildkitd. `v*` tags push

`forge.oddie.app/jlxq0/typst-mcp:<tag>` (and GHCR).

```sh
docker run --rm -p 3000:3000 \
  -e TYPST_MCP_PUBLIC_URL=https://typst-mcp.kampong.social \
  -e TYPST_MCP_TENANT_SALT=... \
  -e TYPST_MCP_SIGNING_SECRET=... \
  -e TYPST_MCP_OIDC_ISSUER=https://login.microsoftonline.com/<tenant-id>/v2.0 \
  -e TYPST_MCP_OIDC_AUDIENCE=api://typst-mcp \
  -v typst-data:/data \
  forge.oddie.app/jlxq0/typst-mcp:v0.1.0
```

## Licence

[MIT](LICENSE).
