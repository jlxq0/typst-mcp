# Release evidence

Current as of 2026-08-23 for `typst-mcp v0.2.0`.

## Source and build

- Release source: tag `v0.2.0`, commit
  `25c9b8ffffe62b76e2217dd5836f9d6ba4956624`.
- Canonical OfficeMaster Freudenberg source:
  `4c99d8ca66677f965930c4d2ef77da7f040a35c6`.
- Locked local gate: formatting, clippy with warnings denied, and 279 tests passed on
  2026-08-23.
- Forge tag run 6634: cargo gate, exact linux/amd64 image build, all ten smoke steps,
  publish, and teardown passed.
- Forge soak run 6633: 10,000 distinct sequential compiles completed in 266 seconds;
  warm RSS 5,894,046 bytes, end RSS 6,025,118 bytes, peak RSS 6,824,133 bytes, zero
  restarts.
- Post-release main run 6644 passed.
- Published image digest:
  `sha256:67b2817da5232e8331fa4c3a43922a07f9ff4b62889568efa0e6531790635285`.

## Configuration and deployment

- `Oddie Apps/typst-mcp-www` supplies distinct 64-byte `hanso` and `release-smoke` REST
  keys; values are not recorded here. The live ExternalSecret is `Ready=True` and
  `SecretSynced`.
- Worker envelope: two concurrent 1 GiB workers, 2 CPU and 3 GiB pod limit.
- Storage: Bound 5 GiB ReadWriteOnce Longhorn PVC; outputs remain re-creatable cache data.
- Metrics: separate port 9090; VictoriaMetrics reports
  `up{namespace="typst-mcp",service="typst-mcp"}=1`.
- OTLP: stable service identity; exporter intentionally disabled because Fondue has no
  trace backend.
- Edge: 32 MB request guard and unbuffered MCP streaming on all three edge nodes.
- Argo CD on 2026-08-23: `typst-mcp`, `network-policies`, and `monitoring` are
  Synced/Healthy. The `typst-mcp` application revision is
  `011cfa491411a66d175af94686eb3aae85985446`.
- Live workload: `1/1` Ready, zero restarts, four templates, version `0.2.0`, and the
  published image digest above.

## Acceptance

- All ten production REST/MCP smoke steps pass against
  `https://typst-mcp.hanso.group`; the most recent rerun was 2026-08-23.
- Claude Desktop completed Entra OAuth, MCP initialization, and `tools/list`; its refreshed
  bearer passed the production MCP smoke.
- A four-page Hanso PDF rendered by production passed manual visual inspection with no
  clipping, overlap, or fallback-font artifacts. All four shipped templates render from
  their fixtures in the locked test gate.
- The production `hanso30` Phoenix release rendered a three-page PDF through the cluster
  Service, proving the internal REST path without the public edge.
- The release security scan completed with no surviving findings. `cargo audit` reported
  no known vulnerabilities; the five accepted unmaintained Typst transitives remain
  documented in `.cargo/audit.toml`; cargo-deny bans/licenses/sources passed.

## Canonical links

- Forge release run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/55`
- Forge soak run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/54`
- Post-release Forge run: `https://forge.oddie.app/jlxq0/typst-mcp/actions/runs/56`
