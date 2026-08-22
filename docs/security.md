# Security model

typst-mcp accepts untrusted Typst source, template archives, assets, metadata and
OAuth traffic. Its security boundary is the long-lived HTTP process plus a fresh,
resource-limited subprocess for every compile.

## Trust boundaries

- MCP callers authenticate with a locally validated Hanso Group Entra token. The
  issuer, audience, tenant, delegated scope or application role, algorithm and signing
  key must all match before a principal exists.
- REST callers authenticate with an exact labelled API key. Keys are at least 32 bytes,
  parsed fail-closed at startup and accepted only in the Authorization header.
- Each principal maps through a keyed HMAC to an opaque tenant id. Every store read,
  list, write and delete takes that tenant explicitly; ids never grant authority.
- The parent validates filenames, bundle counts, aggregate sizes and stored-object
  metadata before it reads asset bytes or starts a compile.
- Compile workers receive paths to a parent-owned private workspace, not bulk input on
  an IPC frame. They start with a cleared environment, inherit no application or cloud
  credentials, enforce a memory ceiling and are killed at the deadline.
- `BundleWorld` resolves only files present in the validated bundle. It exposes no OS
  filesystem, package downloader or network resolver to Typst.
- Output links either require the same authenticated tenant or carry a short-lived HMAC
  signature. Expired links are `410 Gone`; invalid live signatures are `403 Forbidden`.

## Attacks and controls

| Threat | Control and regression evidence |
|---|---|
| Credential disclosure | Secrets are header-only, redacted from parsing errors, excluded from the worker environment, and structurally absent from metrics/audit APIs. `auth`, `principal`, `spawn`, `audit`, and `metrics` tests carry credential canaries. |
| Cross-tenant object access | All object operations require `Principal.tenant`; stored ids are validated as opaque server-generated ids. HTTP and MCP integration tests attempt asset, template, output and signed-link cross-tenant access. |
| Path traversal or archive escape | `BundlePath` rejects absolute, dot, empty, padded, control and exotic segments. Tar members and links are validated before extraction; storage ids are validated before joining a path. Bundle/template/store tests cover traversal tables and hostile ids. |
| Parent-process memory exhaustion | The full asset list is checked for duplicates, count and cumulative metadata size before the first stored byte is read. Request frames and retained OAuth/JWKS state have explicit bounds. |
| Compiler CPU or memory denial of service | One worker per compile, bounded concurrency, per-worker `RLIMIT_AS`, a hard deadline and forced kill. The exact-image smoke kills a finite CPU-bound document and proves a subsequent compile succeeds. |
| Permanent Typst process growth | One-shot workers discard Typst's permanent `FileId` interner and global `comemo` cache after every document. The distinct-filename soak checks the server's long-lived RSS and restart count. |
| OAuth/JWKS amplification | Impossible algorithms fail before provider I/O. JWKS refresh is single-flight with minimum intervals, failure backoff and a bounded negative cache; cached valid keys are never blocked behind provider I/O. |
| Redirect or authorization-code theft | OAuth redirect URIs are exact-string allowlisted at registration, authorization and token exchange; cleartext HTTP is loopback-only and PKCE is preserved. |
| Information disclosure through errors | Domain failures pass through one sanitized `ApiError` classifier for REST and MCP. Internal paths, worker failures and compiler internals do not reach public envelopes. |
| Unbounded disk growth | Per-tenant and global quotas, TTL reaping and oldest-first eviction bound logical storage; the 5 GiB PVC is the final physical ceiling. Outputs are re-creatable cache entries, not records of truth. |

## Operational invariants

- Production runs as distroless UID/GID 65532 with a read-only root filesystem,
  dropped capabilities and RuntimeDefault seccomp.
- Two concurrent 1 GiB workers require a 2 CPU, 3 GiB pod limit. Any change to the font
  set, worker limit or concurrency must rerun the exact linux/amd64 image smoke.
- Prometheus is served on a separate port and is reachable only by VictoriaMetrics.
  Request-derived labels are reduced to a fixed operational vocabulary.
- OTLP export is opt-in. `OTEL_SERVICE_NAME=typst-mcp` is stable; no endpoint is set on
  Fondue until a real trace backend exists, so the process opens no telemetry connection.
- The edge accepts at most 32 MB per request, above the application's 16 MiB upload cap,
  and keeps `flush_interval -1` for MCP streaming.

## Review checklist

Before a release, run the locked fmt/clippy/test gate, dependency audit and deny checks,
the exact-image ten-step smoke, the 10,000-filename soak, and a repository security scan.
Record exact commits, image digest, deployment generation and live acceptance in
`docs/release-evidence.md`.
