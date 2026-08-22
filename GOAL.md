# typst-mcp — Goal

Add an endpoint to Claude Desktop, say *"write this as a letter using the Hanso template"*,
and get back a document that looks like it came from a designer — not like an AI generated
a Word file.

Third sibling to `matrix-mcp` and `jmap-mcp`: Rust, axum + rmcp, fondue, Forgejo CI,
distroless nonroot. Unlike those two it has no upstream backend to delegate auth to, so it
authenticates against Microsoft Entra itself.

---

## 1. What it does

Two front doors, one engine:

| Door | Path | Who | Auth |
|---|---|---|---|
| MCP | `/mcp` | Claude Desktop, Claude Code | OAuth 2.1 bearer, Entra |
| REST | `/api/v1/*` | in-cluster Phoenix apps, scripts | static API key |

A caller picks a **template** — `hanso-invoice`, `freudenberg-letter`, whatever is in the
image — supplies **data**, and gets back:

1. **Diagnostics** — structured, with file, line, column and hints.
2. **A URL** to the PDF, openable with the credential or a short-lived signed link.
3. **A preview PNG**, returned as an MCP image block, so the model can *see* the page and
   fix it before handing it over.

Templates and fonts are **baked** into the image when they are ours and reused (git,
reviewed, permanent) and **uploaded** when they are one-off (a client's letterhead, a brand
typeface, a template Claude just drafted). Uploads are TTL'd and per-caller.

### Current distribution checkpoint (2026-08-22)

`v0.2.0` is released and deployed by digest
`sha256:67b2817da5232e8331fa4c3a43922a07f9ff4b62889568efa0e6531790635285`.
Forge run 6634 built the exact linux/amd64 image, passed all ten smoke steps, published it,
and completed teardown. Production is `1/1` Ready with zero restarts, the 5 GiB RWO volume
is Bound, both ExternalSecrets are synced, and VictoriaMetrics reports the service up.

Claude Desktop completed the production Entra OAuth flow, MCP initialization and
`tools/list`. Its issued bearer then passed the complete live smoke suite, including MCP
preview rendering, REST rendering and download, signed-link tamper rejection, tenant
isolation, positioned diagnostics, timeout kill and recovery. A four-page Hanso document
rendered by production passed visual inspection with no clipping, overlap or font fallback.
The production `hanso30` Phoenix release also rendered a three-page PDF over the cluster
Service, proving the internal REST path and its two-sided Cilium policy without using the
edge. Forge run 6633 completed 10,000 distinct sequential compiles with zero restarts and
flat retained memory. The release security scan has no surviving findings.

## 2. Why in-process Typst rather than shelling out

`typst::World` **is** the sandbox. Implement `source()` and `file()` over an in-memory
bundle and there is no filesystem to escape and no network to reach — you never hand one
over. Shelling out to the CLI gives the compiler a real directory and a real resolver, and
the rest of the project is spent clawing that back.

It also yields `SourceDiagnostic { severity, span, message, hints }` instead of stderr to
regex, and `PdfOptions { timestamp: None }` makes output byte-identical for identical
input, which turns the strongest regression test in the suite into one line.

## 3. Success criteria

| # | Status | Criterion | Verified by |
|---|---|---|---|
| G1 | [x] | Claude Desktop connects, authenticates via Entra, and lists the tools | Production Claude MCP log: OAuth, `initialize`, `tools/list` |
| G2 | [x] | "Use the Hanso template" produces a PDF the owner would send to a client | Four-page production PDF, manually inspected 2026-08-22 |
| G3 | [x] | Claude sees the preview, spots a layout problem, fixes it, re-renders | `tests/mcp.rs` image-block coverage + live MCP preview smoke |
| G4 | [x] | Claude drafts a new template, uses it, and it can be promoted to git unchanged | `tests/mcp.rs` ephemeral upload/render + archive byte round trip |
| G5 | [x] | A heavy finite Typst program is killed at the deadline and the server stays healthy | Sandbox test + exact-image and live smoke step 10 |
| G6 | [x] | `#read("/etc/passwd")` returns "file not found", never content | `tests/sandbox.rs::the_filesystem_is_not_reachable` |
| G7 | [x] | One caller cannot see another's uploads or outputs, by id or URL | HTTP/MCP tenant-isolation tests + live smoke step 8 |
| G8 | [x] | Disk cannot grow without bound under any caller behaviour | Store quota/LRU tests + Bound 5 GiB production PVC |
| G9 | [x] | 10 000 sequential compiles leave the process healthy and flat on memory | `scripts/soak.sh`, Forge run 6633: zero restarts |
| G10 | [x] | A Phoenix app renders a PDF over `/api/v1` without touching the edge | Production `hanso30` release rendered three pages via cluster Service |
| G11 | [x] | `fmt`, `clippy -D warnings`, `test`, `audit`, `deny` all clean; no image ships un-smoked | Forge tag run 6634 + local 279-test gate + audit/deny |

## 4. Non-goals

- **Admin backend.** A web editor that writes Typst is remote code execution with a login
  form. Templates are git; promotion is a code review.
- **`@preview` packages.** The stdlib covers invoices, letters and documentation. No
  downloader exists in the binary.
- **Beta environment.** Test locally, deploy live — same as `matrix-mcp` and `jmap-mcp`.
- **Rate limiting, object storage, multi-replica, server-side URL fetching.**
- **Output durability.** Outputs live 2 h on a single-replica ReadWriteOnce PVC. They
  survive a pod restart or Recreate rollout, but have no durability SLA and remain
  re-renderable cache entries rather than records of truth.

## 5. The two findings that shaped the design

Both surfaced while verifying the Typst 0.15.1 API. Both work perfectly in dev and take the
service down in week three.

**Typst's `FileId` interner is a permanent 16-bit leak.** `typst-syntax/src/path.rs`
`Box::leak`s every distinct `RootedPath`, indexes it with a `NonZeroU16`, and ends in
`.expect("out of file ids")`. Never freed, capped at 65 535 — and with `panic = "abort"`
that aborts the process. Callers name their own files because imports require it.

**`comemo`'s memo cache is global and grows.** `typst-cli` calls `comemo::evict(10)` after
every compile in its watch loop for exactly this reason.

Neither is fixable inside one long-lived process. Both vanish if each compile runs in a
subprocess that exits — which also makes the timeout real, since you can `SIGKILL` a
process and cannot kill a Rust thread. So: **one subprocess per compile**, no pool, no
recycle counters. If the ~100 ms of font indexing per job ever matters, pooling is the
optimisation — but it is an optimisation, not the design.

## 6. Standards position

Current MCP spec is **2026-07-28** (sessions and the `initialize` handshake removed,
`server/discover` mandatory, DCR deprecated). `rmcp` 3.x shipped the same day and lists
`V_2026_07_28` in `KNOWN_VERSIONS`; only its `LATEST` default still points at 2025-11-25
(upstream PR #1105 open). We pin `rmcp` 3.1.2 and set the protocol version explicitly, then
drop the override when upstream catches up.

The protocol prefers pre-registration over DCR, but the clients verified for this service
still require dynamic registration and same-origin authorization endpoints. The proven
deployment therefore fronts Entra with a narrow bridge: RFC 9728 protected-resource
metadata points at this origin, RFC 8414 metadata advertises `/register`, `/authorize`, and
`/token`, `/register` returns one pre-provisioned Entra public client, and the OAuth proxy
uses `{origin}/oauth/callback` upstream. Redirect URIs are exact-string allowlisted. The
access token presented to `/mcp` remains an Entra JWT and is validated locally.

**The identity provider here is the Hanso Group Entra tenant, and only that.** Logto at
`login.kampong.social` is the IdP for the JMAP and Matrix servers; it is never this
server's issuer, and the two must not be mixed in config, docs or 1Password items.

## 7. Target shape

```
                    ┌────────────────────────────────────────────┐
  Claude Desktop ──▶│ :3000  axum                                │
   (Entra OAuth)    │   /mcp                 rmcp, 2026-07-28    │
                    │   /register, /authorize, /token            │
                    │   /oauth/callback      DCR/OAuth → Entra  │
                    │   /api/v1/*            static API key      │
  Phoenix apps ────▶│   /files/*             bearer | signature  │
   (in-cluster)     │   /.well-known/oauth-protected-resource/mcp│
                    │                                            │
                    │  Principal ─▶ tenant = HMAC(salt, sub|key) │
                    │  /data/t_<tenant>/{assets,tpl,out} on PVC  │
                    └──────────────────┬─────────────────────────┘
                                       │ one job, paths not bytes
                    ┌──────────────────▼─────────────────────────┐
                    │ subprocess per compile — SIGKILL on deadline│
                    │   BundleWorld: no fs, no net, in-memory     │
                    └────────────────────────────────────────────┘
```

## 8. Documents

- `.spec/BUILD_SPEC.md` — decisions, modules, the World, auth, config, tests, phases
- `.spec/TOOLS.md` — the 8 MCP tools and the REST surface
- `.spec/DEPLOY_SPEC.md` — image, CI, manifests, DNS, edge, runbook
- `Plan.md` — the ordered task list with quality gates
