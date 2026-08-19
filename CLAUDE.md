# typst-mcp — project notes

Renders branded PDFs with Typst, over MCP (`/mcp`) and a REST API (`/api/v1`). Rust,
axum + rmcp. Typst is linked in-process; every compile runs in a one-shot subprocess.
Specs in `.spec/`, ordered tasks in `Plan.md`, success criteria in `GOAL.md`.

## Current versus target (2026-08-19)

- Current production is `v0.1.8` at `https://typst-mcp.hanso.group`: the compile sandbox,
  Entra validation, same-origin DCR/OAuth bridge, tenant store, signed links, Hanso template,
  asset upload and seven MCP tools are real. `typst_upload_template` is the target eighth
  tool, not an existing route hidden in the docs.
- Current `main` now has path-only worker control frames, parent-owned RAII job workspaces,
  and a compile-only no-store `output=pdf` path; deployed `v0.1.8` predates those changes.
  The same branch also has one sanitized HTTP/MCP domain-error mapping and tenant-bound
  OIDC bearer downloads with signed-link outage fallback. Ephemeral template upload/delete,
  per-job uploaded fonts, metrics/OTLP/audit, and smoke/soak CI remain Plan completion gaps.
- The live store is a 5 GiB ReadWriteOnce PVC on one replica with `Recreate` rollout. It
  survives pod replacement; TTL and LRU quotas govern content, and PVC capacity is the
  final disk bound. It is still a re-creatable cache, not durable business storage.
- OfficeMaster currently has the canonical Hanso library at `typst/hanso.typ` and a pushed
  KSC implementation at `brands/ksc/typst/ksc.typ` on `feat/ksc-typst-template`. Lenno and
  Freudenberg have briefs/masters but no completed Typst library. Preserve the unrelated
  dirty Freudenberg worktree; never fold it into a template sync accidentally.
- `scripts/sync-templates.sh` currently vendors Hanso only. Adding KSC and moving Hanso to
  its final brand-local home are target work; `templates/UPSTREAM` records the source
  commit, and CI drift checking is not complete yet.

## Known Pitfalls

- **Validate an entire asset list before reading any asset bytes.** The render path used
  to load each referenced asset into the long-lived parent process and only later let
  `Bundle::new` enforce duplicate paths, file count, and aggregate size. Repeating one
  valid large asset id therefore multiplied memory outside the compile worker's
  `RLIMIT_AS`. Keep duplicate-id, count, and cumulative metadata-size checks in a
  metadata-only preflight before the first `Store::get`. Found 2026-08-19.

- **A capacity check must decide whether insertion is allowed.** The OAuth pending-state
  table used to remove expired entries at its nominal cap and then insert regardless of
  whether space was freed, so public `/authorize` requests grew it without bound and
  made each later request scan an ever-larger table. After expiry cleanup, reject while
  still at capacity; also bound every attacker-controlled value retained in the table.
  Found 2026-08-19.

- **Reject impossible JWT algorithms before provider I/O, and single-flight JWKS
  refreshes.** An unknown `kid` is attacker-controlled and must not trigger independent
  discovery/JWKS fetches per request. Serialize refreshes, apply a minimum refresh
  interval and failure backoff, and keep the short unknown-key cache bounded in both
  entry count and key-id length so the mitigation cannot become another memory sink.
  Found 2026-08-19.

- **Never hold the JWKS cache lock across provider I/O.** Single-flight refresh ownership
  and cached-key state need separate locks. Otherwise an attacker-triggered unknown-key
  refresh stalls valid tokens whose signing keys are already cached for the full provider
  timeout, converting an upstream degradation into a global authentication outage.
  Found 2026-08-19 during remediation review.

- **Credential parsers must fail closed, not reinterpret malformed entries.** The API
  key parser used to treat malformed `label:secret` entries such as `alice:` as bare
  secrets, turning a configuration mistake into a weak working credential. Require the
  documented labelled form, enforce the 32-byte minimum at startup, reject duplicate
  labels, and ensure parse errors never retain or print a secret. Found 2026-08-19.

- **A subprocess sandbox inherits ambient credentials unless its environment is
  cleared explicitly.** Compile workers need only the framed job and executable path;
  inheriting `TYPST_MCP_*`, cloud, or proxy variables turns a future compiler escape
  into credential exposure. Start from `env_clear()` and restore only the reviewed
  backtrace controls. Filesystem and network isolation remain separate boundaries.

- **Classify domain failures before selecting a transport.** Duplicated REST and MCP
  mappings let bundle caps return the wrong status and made some MCP failures look like
  successes. Keep auth/render/bundle/template/store/link/upload/download failures behind
  `ApiError` and retain the live REST/MCP envelope-parity regression.
  Found 2026-08-19.

- **Typst's `FileId` interner is a permanently-leaked 16-bit counter.**
  `typst-syntax/src/path.rs` `Box::leak`s every distinct `RootedPath`, indexes it with a
  `NonZeroU16`, and ends in `.expect("out of file ids")`. Entries are never freed, so at
  65 535 distinct paths the process panics — and `panic = "abort"` makes that fatal.
  `comemo`'s memo cache is global and grows the same way. Neither is reclaimable inside
  a long-lived process, which is why compiles run in a subprocess that exits after one
  document. **Never use `FileId::unique()`** — it skips the dedup map entirely.

- **Pin `comemo` and `tiny-skia` to whatever typst resolves, and check after every
  bump.** `comemo = "0.4"` looks right and compiles, but typst 0.15 uses 0.5: cargo
  silently links *both*, so `comemo::evict()` clears an empty cache while typst's own
  grows forever. Verify with `cargo tree -p comemo -p tiny-skia` — an "ambiguous
  specification" error means there are two copies. Found 2026-08-15.

- **`typst_render::render` ends in `Pixmap::new(w, h).unwrap()`.** Pixel dimensions come
  from `(pixel_per_pt * size).round().max(1.0) as u32`, and that cast *saturates*, so a
  large enough page reaches `Pixmap::new` with `u32::MAX` and panics. Any change to
  preview sizing must keep `predicted_pixels` mirroring that arithmetic exactly.

- **Clamp, then never re-raise.** `clamp_scale` used to end in `.max(0.01)` to guard
  against a zero scale, which silently discarded the pixel cap for any page longer than
  `max_px / 0.01` points — a 400000pt page rendered at 4000x4000 against a 2000px cap.
  Sanitising the input and bounding the output are two steps; one `max` cannot do both.
  The A4-sized test passed throughout. Found by adversarial review 2026-08-15.

- **Never slice a byte offset from Typst.** Diagnostic spans are byte offsets; slicing
  one that lands inside a multi-byte character panics, and `panic = "abort"` turns that
  into a process abort. Use `Source::lines().byte_to_line()` / `byte_to_column()`. The
  sibling `jmap-mcp` records a real outage from exactly this.

- **Frame limits must be symmetric.** `write_frame` and `read_frame` enforce the same
  `MAX_FRAME_BYTES`. When only the reader checked, a worker could spend a whole compile
  producing a frame the parent then refused, and the failure surfaced far from its cause.

- **Diagnostics live on the error, uniformly.** `CompileError` carries a
  `Vec<Diagnostic>` for every failure kind. An earlier version attached them only to the
  variants that obviously needed them, so warnings gathered before a *late* failure —
  page-cap trip, export error — were silently dropped.

- **`current_exe()` is the test harness inside an integration test.** Spawning it with
  `--compile-worker` re-runs libtest instead of compiling. Use
  `SpawnConfig::for_exe(env!("CARGO_BIN_EXE_typst-mcp"))`.

- **`range(N)` in Typst materialises the whole array.** The runaway-compile
  fixture used `#for i in range(300000000)`, which is a *memory* bomb, not a slow
  computation. Linux enforces the worker's `RLIMIT_AS`, so the worker died allocating
  in under a second and the request came back **500 instead of 504**; macOS does not
  enforce that limit the same way, so it passed locally and failed only in CI — and
  the reaction was to let the docker job publish `if: always()`, which shipped
  v0.1.3/v0.1.4/v0.1.5 through a hole in the gate. A deadline fixture must be
  CPU-bound: nested `range(20000)` loops, not one enormous range. Found 2026-08-17.

- **Typst catches trivially infinite loops itself** (`#while true {}` → "loop seems to
  be infinite"), so it never reaches the deadline. The case the guard cannot catch is a
  heavy *finite* document, and that is what the timeout test must use — measured at
  ~4.3s for 40M loop iterations, well past 30s for 300M.

- **An `#include`d file does not inherit the importer's scope.** A wrapper template's
  body therefore needs its own `#import` prelude, or it loses every binding the template
  provides (brand colours, chart helpers). The prelude shifts line numbers, so
  `templates::SourceMap` subtracts it before diagnostics are returned.

- **MCP 2026-07-28 removed sessions and the `initialize` handshake.** Every request
  carries `_meta` with `io.modelcontextprotocol/protocolVersion` and
  `clientCapabilities`, and a streamable-HTTP POST must additionally send
  `Mcp-Protocol-Version` and `Mcp-Method` headers matching the body — plus `Mcp-Name`
  for `tools/call`. A request missing `_meta` gets `-32602`; one missing the headers
  gets `-32020`. `tests/mcp.rs` encodes the working shape.

- **rmcp's `#[tool_handler]` defaults to `Self::tool_router()` — a call, not the
  field.** Left at the default it rebuilds the entire tool list on every invocation
  and the cached `tool_router` field reads as dead code. Use
  `#[tool_handler(router = self.tool_router)]`.

- **Never cache the caller on an rmcp service.** rmcp builds one service per *session*,
  and a session is not a caller. Read the principal from each request instead: rmcp
  injects the HTTP `Parts` into `RequestContext.extensions`, so
  `ctx.extensions.get::<http::request::Parts>()` reaches what the auth middleware left
  there. An earlier version handed the tenant to the service factory through a shared
  slot, which two concurrent sessions could swap.

- **An Entra v2.0 access token's `aud` is the API's client-ID GUID, never the
  `api://` App ID URI.** The URI form appears only in v1.0 tokens, and our API app has
  `requestedAccessTokenVersion: 2`. Configuring `OIDC_AUDIENCE=api://typst-mcp` alone
  therefore rejects *every* real token, and the symptom is not an error anyone sees: the
  client completes OAuth, gets 401 from `/mcp`, assumes its token is stale, re-runs the
  whole flow, and reports "needs auth, 0 tools" forever. Entra's non-interactive sign-in
  log showed five successful redemptions in sixteen seconds — that retry storm *is* the
  signature. `OIDC_AUDIENCE` is a list now; keep the URI first (it also qualifies bare
  scopes for authorize) and the GUID after it. Found 2026-08-17.

- **rmcp's DNS-rebinding guard defaults to loopback only.**
  `StreamableHttpServerConfig::default()` sets `allowed_hosts` to `localhost`,
  `127.0.0.1`, `::1`, and answers **403** to any other `Host` — *after* the bearer
  token has been accepted, so a client reports "failed to load MCP server / 0 tools"
  and the operator goes looking at auth. No test can catch it by accident: the suite
  and every local run connect over loopback, which is on the default list.
  `mcp_router` now derives the list from `PUBLIC_URL`, which is by definition the
  `Host` clients will send. Found 2026-08-17, one layer behind the audience bug below.

- **`TraceLayer::new_for_http()` logs nothing at INFO.** Its request/response events are
  DEBUG, so the server emitted its startup banner and then went silent, and the OAuth
  incident above had to be reconstructed from Entra's audit logs instead of our own.
  `serve()` now sets `on_response` to INFO explicitly. Log the **path, never the URI** —
  `/oauth/callback` carries an authorization code in its query string.

- **`jsonwebtoken` defaults to 60 seconds of `exp`/`nbf` leeway.** Set
  `validation.leeway` explicitly so the tolerance is a decision rather than an
  inherited default, and remember it when writing a test for an expired token.

## Conventions

- Untrusted paths are interpreted in `bundle.rs` and nowhere else. **Reject, never
  sanitise** — a rewritten hostile path hides the attempt.
- Caller data reaches generated Typst source only through `typst_value.rs`, as typed
  literals. Bare identifiers come from a template-author allowlist, never caller input.
- Store lookups take `(tenant, id)`. There is deliberately no un-scoped `resolve(id)`.

## Gate

```bash
cargo fmt --all --check \
  && cargo clippy --all-targets --all-features --locked -- -D warnings \
  && cargo test --all-features --locked
```

## Verified environment facts (2026-08-15)

- Cluster **fondue**, `KUBECONFIG=~/.kube/config-fondue`. Everything is ArgoCD from
  `forge.oddie.app/oddie-apps/platform.git`; `kubectl set image` gets reverted on sync.
- `gateway/web` has one listener, `http` on port 80. **No in-cluster TLS** — Caddy
  terminates at the edge and routes set `X-Forwarded-Proto: https` themselves.
- MCP vhosts need **`flush_interval -1`** in Caddy or the streamable-HTTP stream buffers
  and the client hangs with no error.
- 1Password: store `onepassword-hanso` reaches vaults `Gruyere` and `Oddie Apps`. There
  is no vault named "Hanso". Registry creds are shared from the `matrix-mcp-www` item.
  The app's own item is **`typst-mcp-www` in `Oddie Apps`**, not Gruyere — the comment in
  `platform/clusters/fondue/typst-mcp/external-secret.yaml` still says Gruyere and is
  wrong. It currently maps only `TENANT_SALT`, `SIGNING_SECRET`, and `OIDC_ISSUER`.
  `OIDC_AUDIENCE`, `OIDC_TENANT_ID`, `DCR_CLIENT_ID`, and the redirect allowlist are
  reviewed public deployment values; mapping `API_KEYS` remains release work.

## Verified environment facts (2026-08-17)

- **The canonical origin is `https://typst-mcp.hanso.group`.** It is the RFC 9728
  `resource` (`{origin}/mcp`), the RFC 8414 `issuer`, and the callback Entra has
  registered — so it is an identity boundary, not a routing detail. The HTTPRoute also
  answers `typst-mcp.kampong.social`; that name is a spare, and nothing may advertise it.
- The Caddy vhost for `typst-mcp.hanso.group` was running on the edge **without being in
  `edge-config/caddy/Caddyfile`**. An `edge-config-sync.sh` run would have deleted it and
  taken OAuth down with it. Now committed, sharing one block with the kampong name.
- Entra (tenant `96e9b6ca-420e-4193-a079-bbf83b313f5f`) has **two** app registrations:
  the API `typst-mcp` (`api://typst-mcp`, appId `57eb9d5b-1bb5-4df0-b62e-e8343e1d2367`,
  delegated scope `render`), and the public client `typst-mcp-public`
  (`10c81c41-1b0d-4954-b855-25fe19d9dbce`, `isFallbackPublicClient: true`) which the API
  pre-authorises and `/register` hands out. Only the second carries redirect URIs.
- Entra v2's `/authorize` **tolerates the RFC 8707 `resource` parameter** — verified by
  request, not by documentation. The proxy passes it through when it names our MCP URL.
