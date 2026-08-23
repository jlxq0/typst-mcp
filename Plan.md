# typst-mcp — Plan

Ordered. Work top to bottom unless told otherwise. Specs: `GOAL.md`, `.spec/BUILD_SPEC.md`,
`.spec/TOOLS.md`, `.spec/DEPLOY_SPEC.md`.

**Gate — run after every task, before checking it off:**

```bash
cargo fmt --all --check \
  && cargo clippy --all-targets --all-features --locked -- -D warnings \
  && cargo test --all-features --locked
```

Green → tick the box and append `<!-- completed YYYY-MM-DD -->`. Red → fix first. Never
start a phase over a red gate.

---

## Phase 0 — Spike & scaffold

- [x] Spike `BundleWorld` on typst **0.15.1**: in-memory source → `%PDF-` out. The 0.15 API
      is `RootedPath::new(VirtualRoot::Project, VirtualPath::new(…)).intern()` — pre-0.15
      examples on the web will not compile. <!-- completed 2026-08-19 -->
- [x] Spike `rmcp` **3.1.2** with the protocol version set explicitly to `V_2026_07_28`
      (it is in `KNOWN_VERSIONS`; only `LATEST` still points at 2025-11-25). Confirm
      `#[tool_router]` and the streamable-HTTP service wiring. <!-- completed 2026-08-19 -->
- [x] `cargo init`; copy `rustfmt.toml`, `deny.toml`, `.gitignore`, LICENSE from `jmap-mcp`.
      <!-- completed 2026-08-19 -->
- [x] `Cargo.toml` per BUILD_SPEC §8. Run `cargo tree -p comemo -p tiny-skia` and pin those
      to whatever typst resolves. <!-- completed 2026-08-19 -->
- [x] `config.rs` + `main.rs`: `serve` / `--compile-worker` dispatch, `/health`, metrics
      listener, graceful shutdown. The metrics listener exposes only `/metrics` and shares
      the server's graceful-shutdown signal. <!-- completed 2026-08-19 -->
- [x] `.forgejo/workflows/ci.yml` cargo gate job (docker job in Phase 8).
      <!-- completed 2026-08-19 -->
- [x] Unit tests: config defaults, missing required var, short salt rejected.
      <!-- completed 2026-08-19 -->

## Phase 1 — Compile core

- [x] `bundle.rs` — path normalisation and caps. **All untrusted-path logic lives here**;
      the future template-tarball path must reuse it. <!-- completed 2026-08-19 -->
- [x] `world.rs` — `BundleWorld`, eagerly populated, no interior mutability. A `file()` miss
      returns `FileError::NotFound`; that line is the filesystem sandbox. Reject
      `VirtualRoot::Package(_)` with a clear diagnostic. <!-- completed 2026-08-19 -->
- [x] `diagnostics.rs` — `SourceDiagnostic` → `{severity,file,line,col,message,hints}` via
      `Source::byte_to_line` / `char_indices`. Never slice a byte offset (`jmap-mcp`
      CLAUDE.md records that outage class). <!-- completed 2026-08-19 -->
- [x] `compile.rs` — `typst::compile::<PagedDocument>` → `typst_pdf::pdf` → preview via
      `typst_render::render`. `PdfOptions { timestamp: None }`. Page cap and preview
      dimension clamp enforced before export. <!-- completed 2026-08-19 -->
- [x] Tests: bundle traversal table; diagnostics offsets (ASCII/multibyte/EOF/zero), compile
      behaviour, and determinism. These live beside the modules rather than in the two
      originally proposed integration-test filenames. <!-- completed 2026-08-19 -->

## Phase 2 — Subprocess & limits  ← the phase that makes it safe

- [x] `worker.rs` — `--compile-worker` entry, length-prefixed JSON framing, `RLIMIT_AS`
      self-imposed, and path-only control frames in both directions. The parent stages a
      validated bundle under opaque numeric names, owns fixed output paths, and removes the
      private workspace on every exit path. <!-- completed 2026-08-19 -->
- [x] `spawn.rs` — spawn from `current_exe()`, `kill_on_drop`, deadline + `SIGKILL`,
      concurrency semaphore, queue then 503. <!-- completed 2026-08-19 -->
- [x] Tests: `tests/sandbox.rs` complete — a finite, CPU-bound runaway is killed **and the
      next compile succeeds**; Typst's own `#while true {}` guard is tested separately; no
      filesystem escape; packages unavailable with no downloader; memory bomb contained.
      <!-- completed 2026-08-19 -->

## Phase 3 — Auth

- [x] `principal.rs` — `Principal`, HMAC tenant derivation, fingerprints, `Zeroizing` keys.
      <!-- completed 2026-08-19 -->
- [x] `oidc.rs` — Entra discovery and JWKS validation, including rotation, single-flight
      refresh, failure backoff, and `iss` / `tid` / `aud` / `scp` / `exp` checks.
      <!-- completed 2026-08-19 -->
- [x] `auth.rs` — constant-time API-key comparison across all entries, no early return.
      <!-- completed 2026-08-19 -->
- [x] `oauth_metadata.rs` — RFC 9728 document + `WWW-Authenticate` with `resource_metadata`
      and `scope`. This is the one thing MCP 2026-07-28 says servers **MUST** implement.
      <!-- completed 2026-08-19 -->
- [x] **Entra + Claude spike** (BUILD_SPEC §5.3): Claude and other native clients were
      verified to require DCR despite the 2026-07-28 preference for pre-registration.
      The service now supplies a bounded redirect-allowlisted DCR shim and proxies
      `/authorize`, `/oauth/callback`, and `/token` to the pre-provisioned Entra public
      client. The result and the required v2.0 GUID audience are recorded in `CLAUDE.md`.
      <!-- completed 2026-08-19 -->
- [x] Auth tests: bad aud/tid/iss/expiry, API key rejected on `/mcp`, key never accepted
      from a query parameter, metadata/DCR shape, and redirect allowlisting.
      <!-- completed 2026-08-19 -->

## Phase 4 — Store & delivery

- [x] `store.rs` — per-tenant opaque ULID-addressed layout for assets / ephemeral templates /
      outputs, `tmp/` + `rename()` atomic writes, TTL reaper, per-tenant **and** global LRU
      quotas, boot-time index scan. Opaque ids are intentional: output groups and mutable
      TTL/metadata do not have a content identity, and draft uploads must not reveal
      cross-tenant equality. <!-- completed 2026-08-19 -->
- [x] `signing.rs` — mint/verify, `v1|` domain separation, `exp` checked first.
      <!-- completed 2026-08-19 -->
- [x] `/files/{tenant}/{job}/{name}` accepting an Entra bearer, API key, or signature.
      Tenant equality is enforced for either bearer; a valid signature remains usable
      during an OIDC provider outage. <!-- completed 2026-08-19 -->
- [x] 410-not-404 on expiry, everywhere. <!-- completed 2026-08-19 -->
- [x] Store/tenancy/signing tests. Coverage includes quota/LRU/restart/expiry, scoped
      deletion, API-key/OIDC/signed downloads, and ephemeral-template tenant isolation,
      expiry and quota enforcement. <!-- completed 2026-08-19 -->

## Phase 5 — Surfaces

- [x] `ApiError` + the single HTTP/MCP mapping (BUILD_SPEC §10). Compile failure is **422
      with diagnostics**, bundle caps are 413, expiry is 410, and internal failures are
      sanitized. REST and MCP parity is exercised over both live transports.
      <!-- completed 2026-08-19 -->
- [x] `api.rs` — `/api/v1/render|compile|templates|assets|fonts|links`, `output=pdf`
      returning bytes and storing nothing. Template tar/gzip upload, lookup/list and
      ephemeral-only deletion are included. <!-- completed 2026-08-19 -->
- [x] `mcp.rs` — `TypstMcpService`, the 8 tools per TOOLS.md, preview as an MCP image block.
      Compile accepts source or multi-file text bundles, main, data, inputs and assets;
      render accepts inputs and checked template-file overrides; asset roles, filters and
      limits are shared across REST/MCP. <!-- completed 2026-08-19 -->
- [x] Uploads: `POST /api/v1/assets` (binary), `POST /api/v1/templates` (tarball — member
      paths through `bundle.rs`; `../` rejected, never sanitised), `typst_upload_template`
      (text only). Template archives are never extracted, gzip is accepted, links/devices
      are rejected, and fixture compiles happen before persistence. <!-- completed 2026-08-19 -->
- [x] MCP round-trip coverage at protocol `2026-07-28`: auth, discovery, protocol pinning,
      the exact eight-tool list, preview image blocks, text-only template upload, tenant
      isolation and rendering by `tpl_…` id. <!-- completed 2026-08-19 -->

## Phase 6 — Content

Canonical libraries for Hanso, KSC, Lenno and Freudenberg live under
**`OfficeMaster/brands/<brand>/typst/`**. Hanso's demos, Figtree copy and figures remain
in `OfficeMaster/typst/`.

- [x] `fonts.rs` — book from embedded + baked dirs plus uploaded font files staged only in
      the requesting job's RAII workspace. Each one-shot worker builds its own book; a real
      Figtree regression proves the next job falls back rather than inheriting it.
      <!-- completed 2026-08-19 -->
- [x] `templates.rs` — resolver over baked names + `tpl_…` ids; `template.toml` with
      `kind`/`wrapper_fn`/`[args]` coercion, `schema.json`, fixtures. `data` validated
      before compiling; values emitted through a typed serialiser, never string-pasted.
      Canonical archive round trips preserve every source byte for git promotion.
      <!-- completed 2026-08-19 -->
- [x] `templates/hanso/` — `hanso.typ` copied in, `template.toml` (`kind = "wrapper"`,
      `wrapper_fn = "hanso-doc"`), `schema.json` derived from the `hanso-doc` signature
      (title, author, date, org, address, phone, email, web, social, bank, footer-style,
      support, confidentiality, theme), `fixture.json` + `fixture.body.typ`.
      <!-- completed 2026-08-19 -->
- [x] **Template home — decided.** Canonical copy lives in
      `OfficeMaster/brands/<b>/typst/`, beside that brand's brief and its Word/PPT masters
      typst-mcp **vendors** them into
      `templates/` in git via `scripts/sync-templates.sh`, which records the OfficeMaster
      commit in `templates/UPSTREAM`. CI re-runs it with `--check` and fails on drift.
      Vendored in git rather than cloned at image build: no Forgejo credentials in the
      build, hermetic and offline-capable images, and drift becomes a visible diff instead
      of a silent difference between two builds. The registry now enforces Hanso, KSC and Lenno
      from a trusted OfficeMaster checkout in CI. Hanso's move is complete at OfficeMaster
      commit `d3aeaaf`; Freudenberg is complete at OfficeMaster commit `4c99d8c`.
      <!-- completed 2026-08-22 -->
- [x] `templates/ksc/`, `templates/lenno/`, `templates/freudenberg/` — port the `hanso.typ`
      pattern against each brief in `OfficeMaster/brands/<b>/README.md` (palettes, fonts,
      light/dark modes are all specified). Each gets `template.toml`, `schema.json`,
      fixtures. KSC is complete from canonical commit `ff0f464`, with safe generic fixture,
      Inter/Passion One plus OFL texts, embedded-font assertions and real rendering. Lenno
      is complete from canonical commit `7063930`, with a realistic operations fixture,
      Space Grotesk/Roboto licence texts, embedded-font assertions and real rendering.
      Freudenberg is complete with Source Sans 3, Roboto Slab, fixture, schema and render coverage.
      <!-- completed 2026-08-22 -->
- [x] `freudenberg/template.toml` carries a `notice`: TheSans is commercial and must never
      be embedded; the logo is Freudenberg's trademark and external distribution needs
      usage rights confirmed. Surfaced by `typst_templates`. <!-- completed 2026-08-22 -->
- [x] Apply the settled Freudenberg typography: **Source Sans 3** for body/headings and
      **Roboto Slab** for the accent face. TheSans remains prohibited; no comparison or
      Fira Sans candidate remains (DEPLOY_SPEC §1.1). <!-- completed 2026-08-22 -->
- [x] `scripts/fetch-fonts.sh` + `fonts/fonts.list` (15 families, DEPLOY_SPEC §1.1) —
      every family pulled with its licence text; a family without one fails the build.
      This supplied the previously missing Figtree, Roboto, Roboto Slab, and Source Sans 3
      families. <!-- completed 2026-08-19 -->
- [x] Tests: render all four from their fixtures; port `the-big-five.typ` as an integration
      fixture (its 5 JPEGs exercise the uploaded-asset path end to end); every brand font
      resolves without fallback; an uploaded font applies to one job only.
      <!-- completed 2026-08-22 -->

## Phase 7 — Observability

- [x] `metrics.rs` (`typst_mcp_*`), `telemetry.rs` (OTLP, no-op unless the endpoint is set),
      `audit.rs` (envelope-only). Histograms retain fixed cumulative buckets rather than
      individual observations. <!-- completed 2026-08-19 -->
- [x] Test that no source text, data value or credential can reach a log line. The audit
      event API has no content-shaped fields, and the captured JSON regression asserts
      source/data/key canaries are absent. <!-- completed 2026-08-19 -->

## Phase 8 — Ship

- [x] Dockerfile per DEPLOY_SPEC §2 (offline-verified fonts stage → builder → distroless
      nonroot). The exact linux/amd64 image built locally and passed the full smoke suite.
      <!-- completed 2026-08-19 -->
- [x] `scripts/smoke.sh` — all 10 steps, exits non-zero on the first failure. It uses a
      disposable real RSA OIDC provider, two API tenants, MCP preview rendering, signed
      delivery, ephemeral isolation, timeout kill and recovery. <!-- completed 2026-08-19 -->
- [x] CI docker job: build to a local tarball, `docker load`, **smoke it, push only on a
      tag**. No image ships without the finite CPU-bound timeout fixture being killed and
      a subsequent compile succeeding. Implemented in the workflow; check only after the
      Forge run proves the loaded-image gate is green. <!-- completed 2026-08-22 -->
- [x] 1Password item `typst-mcp-www` in the **`Oddie Apps`** vault: `TENANT_SALT`,
      `SIGNING_SECRET`, `OIDC_ISSUER`, and the production REST `API_KEYS`. The item and
      live ExternalSecret expose distinct 64-byte `hanso` and `release-smoke` REST keys.
      <!-- completed 2026-08-22 -->
- [x] Manifests in `oddie-apps/platform` → `clusters/fondue/typst-mcp/`:
      `kustomization.yaml`, namespace, deployment (`fsGroup`, RWO Longhorn PVC + `Recreate`,
      CPU limit ≥ concurrency, memory limit > concurrency × worker limit), service,
      external-secret, httproute. The PVC and central
      `clusters/fondue/network-policies/typst-mcp.yaml` Cilium policy are live. The
      deployment has concurrency 2, 1 GiB worker limits, a 2-CPU/3-GiB pod limit, the
      metrics port, OTLP settings, and a two-sided Hanso production REST policy.
      <!-- completed 2026-08-22 -->
- [x] ArgoCD `Application` at `clusters/fondue/apps/typst-mcp.yaml`.
      <!-- completed 2026-08-19 -->
- [x] DNS: `typst-mcp.hanso.group` A/AAAA → `203.24.209.8` /
      `2001:df7:2b40::8` in `dns-primary/zones/hanso.group.zone`, with the SOA serial
      bumped. <!-- completed 2026-08-19 -->
- [x] Edge: Caddy vhost, `bind 0.0.0.0` / `2001:df7:2b40::8`,
      **`flush_interval -1`** (mandatory or the MCP stream
      buffers and hangs), `request_body max_size` > `MAX_UPLOAD_BYTES`; sync via
      `edge-config-sync.sh`. The vhost and 32 MB upload-size guard are live on all three
      edge nodes with streaming enabled.
      <!-- completed 2026-08-22 -->
- [x] Deploy through Forge-hosted git/image repositories and the ArgoCD application;
      `scripts/smoke.sh` green against the live URL; connect Claude Desktop. Production
      `v0.2.0` is healthy on the published digest (`1/1`, zero restarts); all ten live
      smoke steps pass, and Claude Desktop completes Entra OAuth, initialization and
      `tools/list` against the production MCP endpoint.
      <!-- completed 2026-08-22 -->

## Phase 9 — Harden

- [x] `scripts/soak.sh` — 10 000 sequential compiles with distinct file names; RSS flat,
      no restart. The measured harness and nightly workflow exist and pass short local
      binary/image runs; check after one complete 10,000-compile run.
      <!-- completed 2026-08-22 -->
- [x] `cargo audit` + `cargo deny check bans licenses sources` clean (with the documented
      accepted unmaintained Typst transitives, and no known vulnerabilities).
      <!-- completed 2026-08-19 -->
- [x] Threat-model pass: no credential in a log or URL, no un-scoped store lookup anywhere,
      every id validated before path construction, tarball extraction cannot escape.
      <!-- completed 2026-08-22 -->
- [x] README, `docs/`, CLAUDE.md "Known Pitfalls" seeded with the `FileId` interner and the
      `comemo` cache. <!-- completed 2026-08-22 -->
- [x] Verify every GOAL.md §3 criterion G1–G11 and tick them off there.
      <!-- completed 2026-08-22 -->

---

## Deferred (do not drift into these)

- Kanjo and the other products — no brand brief exists; revisit when one does.
- Worker pooling — only if per-job font indexing shows up in `compile_duration_seconds`.
- OIDC providers beyond Entra — the config is generic; nothing else to do until needed.
- Admin backend, `@preview` packages, rate limiting, object storage, multi-replica,
  server-side URL fetching, HTML export.
- Bumping `rmcp` past the explicit `V_2026_07_28` pin once upstream PR #1105 lands.
- Porting `matrix-mcp` / `jmap-mcp` from rmcp 1.7 to 3.x — owner is doing that.
