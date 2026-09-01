# Known Pitfalls

- **`GITHUB_REF` does not identify a trigger, so a ref-only guard does not name one
  writer.** The buildcache export was guarded on `refs/heads/main` alone, which reads as
  "only the main build writes". `schedule` and `workflow_dispatch` carry the same ref, so
  it admitted three writers, and Forgejo's default auto-cancel covers `push` and
  `pull_request` (synchronize) **only**: a cron run overlapping a merge build is neither
  cancelled nor cancelling. Two concurrent exports to one unqualified `:buildcache` ref
  lose a blob write and fail with `error writing layer blob: unknown`, **after the image
  has already been pushed**. Test the event as well as the ref. Verified by exercising the
  condition over all five trigger cases rather than by reasoning about it: `main`+`push`
  exports before and after, `main`+`schedule` and `main`+`workflow_dispatch` change from
  export to import-only, tag and pull-request are unaffected. The `main`+`push` row is the
  control; without it the change is indistinguishable from having disabled the export.
  Not fixed with a top-level `concurrency:` block, which would serialise the whole run,
  `cargo` included, on a shared-capacity runner to solve what one condition closes, and
  **job-level `concurrency:` is silently ignored on this Forgejo** so the block is one
  refactor away from doing nothing while still parsing. `jlxq0/typst-mcp#18`,
  `jlxq0/mantis#32`. Found 2026-09-02.

- **`main` is protected, and `CI / docker` is deliberately not a required context.** The
  rule requires `CI / cargo*` only, with `enable_push=false`, `apply_to_admins=true`,
  `required_approvals=0`. Do not "complete" it by adding the docker context: **a job
  skipped because the job it `needs:` failed still posts `success` to the commit status.**
  Measured here on three commits, cargo and docker one second apart, with **no docker task
  in the run at all**:

  | commit | `CI / cargo` | `CI / docker` | docker tasks that ran |
  |---|---|---|---|
  | `45b68251` | failure 02:52:14Z | **success** 02:52:15Z | none |
  | `d8e59351` | failure 02:40:36Z | **success** 02:40:36Z | none |
  | `54de8c40` | failure 02:27:59Z | **success** 02:27:59Z | none |

  So requiring `CI / docker` builds a gate that is green *because the work did not happen*,
  which is worse than not requiring it: it reads as broader coverage while being satisfied
  by the failure it was meant to catch. `docker` still runs and still gates a release,
  building the exact image and smoking it before publishing. It just cannot be a required
  status while it carries a `needs:`.

  The `*` is also load-bearing. Contexts carry an event suffix: a pull-request head posts
  `CI / cargo (pull_request)` and a branch push posts `CI / cargo (push)`. A literal
  string matches one and silently never matches the other, which is an unarmed gate that
  reads as armed.

  **Keep `pull_request:` bare in `.forgejo/workflows/ci.yml`.** The rule depends on two
  properties of that file, and neither is visible from the protection settings. First, the
  `cargo` job carries no `needs:` and no job-level `if:`, so it always runs and its status
  is always the real result. That is why it is safe to require and `docker` is not. Second,
  `pull_request:` has no `paths:`, `paths-ignore:`, `branches:` or `types:` filter, so
  every PR produces `CI / cargo (pull_request)`. Adding one would make a filtered-out PR
  produce no required context at all, and the gate becomes permanently unsatisfiable with
  **nothing in the protection settings having changed**: a merge blocked forever by a line
  in a workflow file nobody connects to it. The `branches: [main]` filter in that block is
  under `push:`, where it cannot affect a PR head.

  Sampled statuses cannot establish this. They say the contexts *have been* produced; the
  `on:` block and the absence of `needs:` say they *must be*.

  A related consequence worth knowing rather than fixing: `cargo` has two step-level
  `if: github.event_name != 'pull_request'` conditions, so the OfficeMaster checkout and
  `scripts/sync-templates.sh --check` do not run on a PR. The job still reports its real
  result, so the gate stays satisfiable, but `CI / cargo (pull_request)` is a weaker check
  than `CI / cargo (push)` and template drift is not covered by the required status.

  Verified 2026-08-27 by pushing rather than by reading the rule back, and with an accepted
  push beside the refused one so the refusal is attributable to the rule rather than to a
  dead token, a wrong remote or a network fault. One commit, two destinations, after
  asserting `git merge-base --is-ancestor origin/main HEAD` so neither could be refused by
  git as a non-fast-forward before reaching the hook:

      git push origin "${probe}:refs/heads/probe/armed-control"   exit 0, * [new branch]
      git push origin "${probe}:refs/heads/main"                  exit 1, ! [remote rejected]
                                                                  pre-receive hook declined

  Brace the refspec. Unbraced, zsh reads `"$probe:refs/..."` as the `:r` history modifier
  and pushes something else. Read the remote's own line, not the exit code. Found
  2026-08-27.

- **A suppression that two places grant is a suppression neither place can remove.**
  `cargo audit` was invoked with `--ignore RUSTSEC-2026-0194 --ignore RUSTSEC-2026-0195`
  in `.forgejo/workflows/ci.yml` while `.cargo/audit.toml` listed the same two. `--ignore`
  suppresses independently of the file rather than being read from it, so the copies were
  not redundant: with the file's `ignore` list emptied, the old CI line still exited 0 and
  a bare `cargo audit` exited 1. Deleting the documented, commented, reviewable entry would
  have changed nothing observable, and the next `quick-xml` bump would have kept a live
  advisory suppressed with nobody able to tell. Keep suppressions in `.cargo/audit.toml`
  alone and run `cargo audit` bare, so removing an entry is what turns the gate red. The
  general form: when a control is granted in two places, removing it from one is not a
  measurement, and the copy you are looking at is usually the one that does nothing.
  Found 2026-08-26.

- **A test named after the running system that takes no input from it is a comment with
  a `#[test]` on it.** `deployed_allowlist_parses` asserted "the exact set the deployment
  ships" with nine entries while the live container env carried seven — a second loopback
  path missing, three private-use entries listed that production does not have. It stayed
  green, and could not have done otherwise: `raw` was a literal and the length assertion
  counted what that literal parsed to, so the deployment was never an input. Renaming it
  to a dated snapshot does not fix that, it only stops it lying. Read an allowlist off the
  thing enforcing it — `kubectl -n typst-mcp get deploy -o jsonpath=...` over the container
  env — and treat any in-repo copy as undated until you have. The same fixture in
  `caldav-mcp`, `carddav-mcp` and `jmap-mcp` matched their deployments on 2026-08-25, so
  accuracy here is luck, not coverage. The cross-repo decision lives in
  https://forge.oddie.app/jlxq0/caldav-mcp/issues/7. Found 2026-08-25.

- **A loopback redirect URI must match on any port (RFC 8252 §7.3).** `validate_redirect_uri`
  carried the loopback carve-out for the *scheme* (cleartext `http` on loopback only) but
  `is_allowed_redirect_uri` still demanded exact string equality, so an allowlisted
  `http://localhost:8787/callback` could never match Claude Code CLI, which binds a random
  free port per session. The symptom is not a matcher error anyone reads: DCR answers
  `400 unregistered redirect_uri` and native loopback clients are permanently locked out.
  Relax the port for cleartext loopback entries only, and keep scheme, host, path and
  query exact — `/callback` must not start matching `/oauth/callback`, `localhost` is not
  `127.0.0.1`, and relaxing the port on `https://claude.ai/...` would be a real hole.
  Found 2026-08-25.

- **Validate an entire asset list before reading any asset bytes.** The render path used
  to load each referenced asset into the long-lived parent process and only later let
  `Bundle::new` enforce duplicate paths, file count, and aggregate size. Repeating one
  valid large asset id therefore multiplied memory outside the compile worker's
  `RLIMIT_AS`. Keep duplicate-id, count, and cumulative metadata-size checks in a
  metadata-only preflight before the first `Store::get`.

- **Do not retain anonymous OAuth authorization state in process memory.** Even a hard
  cap lets public `/authorize` traffic monopolize every slot and deny real logins. Carry
  redirect, client state and S256 challenge in an expiring HMAC-authenticated state value;
  likewise broker the upstream code in an expiring authenticated value. Bind the client
  id, redirect URI and verifier before forwarding any token request.

- **Reject impossible JWT algorithms before provider I/O, and single-flight JWKS
  refreshes.** An unknown `kid` is attacker-controlled and must not trigger independent
  discovery/JWKS fetches per request. Serialize refreshes, apply a minimum refresh
  interval and failure backoff, and keep the short unknown-key cache bounded in both
  entry count and key-id length so the mitigation cannot become another memory sink.

- **Never hold the JWKS cache lock across provider I/O.** Single-flight refresh ownership
  and cached-key state need separate locks. Otherwise an attacker-triggered unknown-key
  refresh stalls valid tokens whose signing keys are already cached for the full provider
  timeout, converting an upstream degradation into a global authentication outage.

- **Credential parsers must fail closed, not reinterpret malformed entries.** The API
  key parser used to treat malformed `label:secret` entries such as `alice:` as bare
  secrets, turning a configuration mistake into a weak working credential. Require the
  documented labelled form, enforce the 32-byte minimum at startup, reject duplicate
  labels, and ensure parse errors never retain or print a secret.

- **API-key secrets must be unique, not only their labels.** Authentication maps a secret
  to a tenant label. Allowing the same secret under two labels makes the later mapping win
  and silently aliases callers into the wrong tenant. Reject duplicate secret digests at
  startup without retaining or printing the secret.

- **Storage quota checks and commits are one serialized transaction.** Concurrent writes
  used to observe the same byte total and all pass the tenant limit. Hold the index lock
  through check, atomic write, metadata update and accounting; persist cumulative bytes
  for multi-file outputs, recompute them on restart, and cap entry counts because empty
  payloads still consume metadata, directories and memory.

- **Bound work done by archive and schema libraries before handing them attacker input.**
  Tar GNU/PAX extension records and gzip expansion happen before member-size checks, and
  collecting every JSON-schema error grows independently of the request body. Limit total
  decompressed archive bytes including metadata, and cap both schema-error count and
  rendered diagnostic bytes.

- **In-memory MCP sessions need an admission ceiling.** rmcp's local manager otherwise
  accepts sessions until the process runs out of memory. Serialize session creation,
  reject at a configured global cap, retain rmcp's initialization and idle timeouts, and
  release capacity on close.

- **A subprocess sandbox inherits ambient credentials unless its environment is
  cleared explicitly.** Compile workers need only the framed job and executable path;
  inheriting `TYPST_MCP_*`, cloud, or proxy variables turns a future compiler escape
  into credential exposure. Start from `env_clear()` and restore only the reviewed
  backtrace controls. Filesystem and network isolation remain separate boundaries.

- **Process metrics must aggregate observations instead of retaining samples.** A
  Prometheus histogram that stores every duration or byte count in a vector grows for
  the full process lifetime under normal traffic. Keep only the fixed cumulative
  bucket counters, total count, and sum; test that retained state stays constant as the
  observation count grows.

- **Length and character bounds do not make request-derived telemetry labels safe.** A
  short alphanumeric template name can still be customer data or a credential. Reduce
  request-derived metric and audit labels to a fixed operational vocabulary, validate
  opaque server-generated ids before emitting them, and exercise every string-shaped
  audit field with content and credential canaries.

- **Expired and invalid signed links are different public errors.** Expiry is actionable
  and returns `410 Gone` consistently with stored-object expiry; a bad signature on a
  still-live link is `403 Forbidden`. Smoke tests and runbooks must not collapse both into
  403 or they contradict the API contract they are meant to verify.

- **Worker memory must be tested in the final image with the full font set.** Unit tests
  and an idle health check do not load the same address space as a branded compile. The
  linux/amd64 image with all 15 families died at a 512 MiB `RLIMIT_AS` under arm emulation
  and passed at 1 GiB; keep the release smoke on the exact image and size the pod above
  `concurrency × worker limit` plus the server.

- **Classify a domain failure once, before choosing HTTP or MCP transport.** Separate
  surface mappings made an oversized bundle a REST 422 while the specification required
  413, and some MCP failures were returned as success-shaped JSON. Route auth, render,
  bundle, template, store, link, upload, and download failures through `ApiError`; keep a
  live REST/MCP parity test so status, code, message, and diagnostics cannot drift again.

- **Release scripts must match both the tools and network topology of the actual CI
  runner.** The OIDC smoke helper used `xxd`, which was available on the development
  host but absent from the Forge Ubuntu job container. After that was removed, the
  remote DinD container started but its published port was probed on the job container's
  unrelated loopback interface. Reuse `python3`, already required by the workflow, keep
  command preflights synchronized, and probe remote-daemon ports through the issued
  daemon address while publishing on a reachable interface. Gate disposable dependency
  readiness explicitly before the application can cache a legitimate startup failure.
  A remote daemon also cannot bind-mount a job-container path; stream generated fixtures
  into a daemon-owned volume and mount that volume read-only instead.

- **Debug and release workers do not share a memory ceiling.** A five-image branded
  document fits the stripped 1 GiB production worker but exceeds that ceiling in a
  Linux debug integration-test binary and surfaces only as a sanitized HTTP 500. Give
  heavyweight functional fixtures explicit debug headroom, keep exact-image smoke as
  the production-memory gate, and do not raise the smaller ordinary-test default.

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

## Design rationale

Kept because none of it is readable from the code, and each is a decision somebody would
otherwise reverse on reasonable-looking grounds.

- **`typst::World` is the sandbox, which is why Typst is linked in-process rather than
  shelled out to.** Implementing `source()` and `file()` over an in-memory bundle means
  there is no filesystem to escape and no network to reach, because neither is ever handed
  over. The CLI would be given a real directory and a real resolver, and the rest of the
  project would be spent clawing that back. It also yields structured
  `SourceDiagnostic { severity, span, message, hints }` rather than stderr to regex, and
  `PdfOptions { timestamp: None }` makes output byte-identical for identical input, which
  is what lets the strongest regression test in the suite be one line.

- **One subprocess per compile, no pool and no recycle counter.** The `FileId` interner and
  the `comemo` cache above are both unfixable inside a long-lived process and both vanish
  if the process exits after one document. It also makes the timeout real, since a process
  can be `SIGKILL`ed and a Rust thread cannot. Roughly 100 ms of per-job font indexing is
  the price, and pooling would be an optimisation rather than the design.

- **Store ids are opaque on purpose.** Output groups and mutable TTL/metadata have no
  content identity, and content-addressed draft uploads would reveal cross-tenant equality.

- **A signed link stays usable during an OIDC provider outage.**
  `/files/{tenant}/{job}/{name}` accepts an Entra bearer, an API key or a signature, and
  tenant equality is enforced for either bearer. The signature path is deliberately not
  gated on the provider being reachable.

- **The identity provider is the Hanso Group Entra tenant and only that.** Logto at
  `login.kampong.social` is the IdP for the JMAP and Matrix servers. It is never this
  server's issuer, and the two must not be mixed in config, docs or 1Password items.

- **Outputs are re-renderable cache entries, not records of truth.** They live 2 h on a
  single-replica ReadWriteOnce PVC and survive a pod restart or `Recreate` rollout, with no
  durability SLA. Anything needing one is in the wrong place.

- **There is no beta environment, by choice.** Test locally, deploy live, as with
  `matrix-mcp` and `jmap-mcp`.

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
  The app's own item is **`typst-mcp-www` in `Oddie Apps`**, not Gruyere. Its live
  ExternalSecret maps `TENANT_SALT`, `SIGNING_SECRET`, `OIDC_ISSUER`, and `API_KEYS` and is
  `Ready=True` / `SecretSynced`.
  `OIDC_AUDIENCE`, `OIDC_TENANT_ID`, `DCR_CLIENT_ID`, and the redirect allowlist are
  reviewed public deployment values.

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
