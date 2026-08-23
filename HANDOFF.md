# typst-mcp handover

Updated: 2026-08-23 SGT

## Stop point

- Branch: `main`, tracking Forge `origin/main`.
- `main` is the only local and Forge branch. The dead alternate `feat/dcr-shim` branch and
  its stale worktree registration were removed during wind-down.
- Release: `v0.2.0` from `25c9b8ffffe62b76e2217dd5836f9d6ba4956624`.
- Production: `https://typst-mcp.hanso.group`, healthy, `1/1` Ready, zero restarts, four
  templates, and image digest
  `sha256:67b2817da5232e8331fa4c3a43922a07f9ff4b62889568efa0e6531790635285`.
- Plan and GOAL are complete. There is no feature or remediation in flight.

## Completed release evidence

- Locked gate: `cargo fmt --all --check`, clippy with warnings denied, and all 279 tests
  pass.
- Forge tag run 6634 built and smoked the exact linux/amd64 image before publishing it.
- Forge soak run 6633 completed 10,000 distinct compiles with zero restarts and stable
  retained memory.
- All ten live REST/MCP smoke steps pass against production.
- Claude Desktop completes Entra OAuth, MCP initialization, and tool discovery.
- Production Hanso PDF visual acceptance and the internal `hanso30` Phoenix render both
  pass.
- Security scan: no surviving findings. Dependency audit: no known vulnerabilities; five
  accepted unmaintained Typst transitives remain documented.
- Detailed evidence is in `docs/release-evidence.md`.

## In flight

None.

## Broken

No known product, CI, deployment, authentication, metrics, or storage failure.

## Needs Julian

Nothing specific. The current release and plan are complete. Anything in Plan's Deferred
section needs an explicit new decision and plan item before implementation.

## Exact next step

For the next change, start from current Forge `main`, read this file and `Plan.md`, add one
explicitly scoped unchecked Plan item, then implement and verify only that item:

```bash
cd /Users/jl/Code/jlxq0/typst-mcp
git pull --ff-only origin main
git status --short --branch
sed -n '1,240p' HANDOFF.md
sed -n '1,320p' Plan.md
```

Do not deploy an unchanged image merely to reproduce this checkpoint. For a source or
image change, run the full locked gate, push to Forge, require the exact-image smoke, pin
the published digest through `oddie-apps/platform`, then smoke the public endpoint with
real credentials.

## Local-only retained files

- `.spec/` remains intentionally gitignored because it is the internal build/design source;
  it is retained because it is not reconstructable from tracked files.
- Build output and prior `.session/` continuity files were removed during wind-down.
