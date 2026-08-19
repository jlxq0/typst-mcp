# Known Pitfalls

- **Validate an entire asset list before reading any asset bytes.** The render path used
  to load each referenced asset into the long-lived parent process and only later let
  `Bundle::new` enforce duplicate paths, file count, and aggregate size. Repeating one
  valid large asset id therefore multiplied memory outside the compile worker's
  `RLIMIT_AS`. Keep duplicate-id, count, and cumulative metadata-size checks in a
  metadata-only preflight before the first `Store::get`.

- **A capacity check must decide whether insertion is allowed.** The OAuth pending-state
  table used to remove expired entries at its nominal cap and then insert regardless of
  whether space was freed, so public `/authorize` requests grew it without bound and
  made each later request scan an ever-larger table. After expiry cleanup, reject while
  still at capacity; also bound every attacker-controlled value retained in the table.

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

- **A subprocess sandbox inherits ambient credentials unless its environment is
  cleared explicitly.** Compile workers need only the framed job and executable path;
  inheriting `TYPST_MCP_*`, cloud, or proxy variables turns a future compiler escape
  into credential exposure. Start from `env_clear()` and restore only the reviewed
  backtrace controls. Filesystem and network isolation remain separate boundaries.

- **Classify a domain failure once, before choosing HTTP or MCP transport.** Separate
  surface mappings made an oversized bundle a REST 422 while the specification required
  413, and some MCP failures were returned as success-shaped JSON. Route auth, render,
  bundle, template, store, link, upload, and download failures through `ApiError`; keep a
  live REST/MCP parity test so status, code, message, and diagnostics cannot drift again.
