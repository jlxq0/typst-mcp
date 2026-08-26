# Release evidence

Current as of 2026-08-26 for `typst-mcp v0.2.1`.

Each claim below says when it was measured. Where a v0.2.0 measurement has not been
repeated for v0.2.1 it is marked as such rather than restated as current — v0.2.1 changed
one file of authorization logic, so most of the v0.2.0 evidence still describes the
shipped artefact, but a dated measurement and an inherited one are not the same thing.

## Source and build

- Release source: tag `v0.2.1`, commit
  `cef453a83fa4a84fc8306634e79fa2d500fcb51d`.
- Published image digest:
  `sha256:0b3da4c968bd91f470fd74f2225682da3b922845d6fa2dc91bb844f5bad216cb`.
  Confirmed against the registry on 2026-08-26: both `v0.2.1` and `latest` resolve to it.
  The v0.2.0 digest was
  `sha256:67b2817da5232e8331fa4c3a43922a07f9ff4b62889568efa0e6531790635285`.
- Canonical OfficeMaster Freudenberg source:
  `4c99d8ca66677f965930c4d2ef77da7f040a35c6` (unchanged since v0.2.0).
- Forge run 75, the `v0.2.1` tag: `cargo` and `docker` both succeeded. The `docker` job
  builds the exact linux/amd64 image, runs all ten smoke steps against it, and only then
  tags and pushes that same local image — there is no second build after the smoke gate.
- Forge run 76, scheduled 2026-08-26 on the same commit `cef453a`: `cargo` and `docker`
  both succeeded. Because the event is `schedule`, this run also executed the
  10,000-distinct-filename soak, so the soak has been repeated on the released commit.
  Its RSS figures were not captured into this document; run 76's log holds them.
- Not repeated for v0.2.1: the v0.2.0 local gate figure of 279 tests on 2026-08-23, and
  the v0.2.0 soak's recorded numbers (10,000 compiles in 266 s; warm RSS 5,894,046 bytes,
  end 6,025,118, peak 6,824,133, zero restarts).

## Configuration and deployment

Verified 2026-08-26 unless noted.

- Live workload: `1/1` Ready, zero restarts, `/health` reports version `0.2.1` and four
  templates.
- Running image, read from the pod's `app` container (**not** `containerStatuses[0]`, and
  not a container named `typst-mcp` — every deployed MCP names its container `app`, and
  the wrong name prints a pod name followed by an empty field rather than an error):
  `forge.oddie.app/jlxq0/typst-mcp@sha256:0b3da4c9…`, matching the digest the tag build
  pushed.
- Argo CD: `typst-mcp` is Synced/Healthy at revision
  `75e84c08f09a9d45b98f5a6dc01aaa94529f469f`.
- Storage: PVC `typst-mcp-data`, Bound, 5 GiB ReadWriteOnce Longhorn. Outputs remain
  re-creatable cache data, not durable business storage.
- Secrets: ExternalSecrets `typst-mcp-secrets` and `forge-secret` are both
  `SecretSynced` / `Ready=True` from the `onepassword-hanso` ClusterSecretStore.
  `Oddie Apps/typst-mcp-www` supplies distinct 64-byte `hanso` and `release-smoke` REST
  keys; values are not recorded here.
- Worker envelope: two concurrent 1 GiB workers, 2 CPU and 3 GiB pod limit.
- Metrics: separate port 9090; VictoriaMetrics reported
  `up{namespace="typst-mcp",service="typst-mcp"}=1` on 2026-08-23.
- OTLP: stable service identity; exporter disabled by choice because Fondue has no trace
  backend.
- Edge: 32 MB request guard and unbuffered MCP streaming on all three edge nodes
  (2026-08-23).

## Acceptance

### v0.2.1's own change, verified against production 2026-08-26

The RFC 8252 §7.3 loopback fix (issue #2) was checked against the deployed service, driven
at the allowlist the deployment actually enforces — read off the container environment
rather than from an in-repo copy, whose only loopback entries are
`http://localhost:8787/callback` and `http://localhost:8787/oauth/callback`. Each row is a
real `GET /authorize`; only `redirect_uri` varies.

| `redirect_uri` | result |
|---|---|
| `http://localhost:8787/callback` (exact entry) | 303 |
| `http://localhost:54321/callback` | 303 |
| `http://localhost:54321/oauth/callback` | 303 |
| `http://127.0.0.1:54321/callback` | 400 `unregistered redirect_uri` |
| `http://localhost:54321/wrong` | 400 `unregistered redirect_uri` |
| `https://localhost:54321/callback` | 400 `unregistered redirect_uri` |
| `http://evil.example.com:54321/callback` | 400 `unregistered redirect_uri` |

The four refusals carry the evidence. Three acceptances alone would also be produced by a
matcher that accepts everything, so they cannot distinguish a working fix from a hole.

### Inherited from v0.2.0, measured 2026-08-22/23, not repeated

- All ten production REST/MCP smoke steps passed against
  `https://typst-mcp.hanso.group`; most recent rerun 2026-08-23.
- Claude Desktop completed Entra OAuth, MCP initialization and `tools/list`; its refreshed
  bearer passed the production MCP smoke.
- A four-page Hanso PDF rendered by production passed manual visual inspection with no
  clipping, overlap or fallback-font artifacts. All four shipped templates render from
  their fixtures in the locked test gate.
- The production `hanso30` Phoenix release rendered a three-page PDF through the cluster
  Service, proving the internal REST path without the public edge.
- The release security scan completed with no surviving findings.

### Dependency audit

`cargo audit` reports no known vulnerabilities and five accepted unmaintained Typst
transitives. Suppressions live in `.cargo/audit.toml` and nowhere else; CI runs a bare
`cargo audit` so that deleting an entry is what turns the gate red. cargo-deny
bans/licenses/sources pass.

## Canonical links

- v0.2.1 tag run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/75`
- Scheduled run on the released commit, including the soak:
  `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/76`
- v0.2.0 release run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/55`
- v0.2.0 soak run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/54`
