# Release evidence

This file records distribution evidence for v0.2.0. Values are filled only after the
named surface has been verified; local success is not presented as deployment proof.

## Source and build

- Release source: pending final commit and tag
- Canonical OfficeMaster Freudenberg source: `4c99d8ca66677f965930c4d2ef77da7f040a35c6`
- Local locked gate: 270 tests passed on 2026-08-22
- Forge cargo job: pending final tag
- Forge exact-image smoke: pending final tag
- Published image digest: pending

## Configuration and deployment

- REST keys: two labelled 32-byte keys in `Oddie Apps/typst-mcp-www`; values are not recorded
- Worker envelope: 2 concurrent workers, 1 GiB each, 2 CPU and 3 GiB pod limit
- Metrics: separate port 9090, VictoriaMetrics-only ingress and scrape
- OTLP: stable service identity; exporter intentionally disabled because Fondue has no trace backend
- Edge: 32 MB request guard and unbuffered streaming verified on all three edge nodes
- GitOps commit and Argo CD revision: pending
- Live image, rollout, routes and metrics: pending

## Acceptance

- Live ten-step REST/MCP smoke: pending
- Claude Desktop Entra connection and tool listing: pending
- Hanso/Freudenberg visual PDF acceptance: pending
- 10,000 distinct-filename soak: pending
- Security scan and threat-model review: pending
