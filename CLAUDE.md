# typst-mcp — project notes

Renders branded PDFs with Typst, over MCP (`/mcp`) and a REST API (`/api/v1`). Rust,
axum + rmcp. Typst is linked in-process; every compile runs in a one-shot subprocess.
Specs in `.spec/`, ordered tasks in `Plan.md`, success criteria in `GOAL.md`.

## Known Pitfalls

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
  is no vault named "Hanso". App item goes in **Gruyere**; registry creds are shared from
  the `matrix-mcp-www` item.
