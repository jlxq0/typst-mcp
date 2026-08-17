//! What a hostile or runaway document must not be able to do.
//!
//! These run against the real spawn path — a genuine subprocess, a genuine deadline,
//! a genuine kill — because that is the only configuration that proves anything. An
//! in-process compile would pass most of these while leaving the actual server
//! vulnerable.
//!
//! The recurring assertion is not just "the request failed" but "and the service
//! still works afterwards". A sandbox that contains one bad document by wedging the
//! server has not contained anything.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, MutexGuard};
use typst_mcp::protocol::{Job, JobContent, JobFile, JobLimits, JobResult};
use typst_mcp::spawn::{CompileService, SpawnConfig, SpawnError};

/// Serialises the CPU- and memory-hungry tests.
///
/// libtest runs tests in parallel, and several of these deliberately saturate a core
/// or allocate to a hard cap. Left to overlap they starve each other, and a trivial
/// compile that takes 30ms alone can miss a ten-second deadline — which produces a
/// failure that says nothing about the code. (That starvation is real in production
/// too, which is why the deployment budgets a CPU limit at or above the concurrency
/// cap; here it is just noise.)
async fn exclusive() -> MutexGuard<'static, ()> {
    static HEAVY: OnceLock<Mutex<()>> = OnceLock::new();
    HEAVY.get_or_init(|| Mutex::new(())).lock().await
}

/// A single-source job with no preview, so tests measure compilation and nothing else.
fn job(source: &str) -> Job {
    Job {
        main: "main.typ".into(),
        files: vec![JobFile {
            path: "main.typ".into(),
            content: JobContent::Text {
                text: source.into(),
            },
        }],
        inputs: BTreeMap::new(),
        font_dirs: vec![],
        limits: JobLimits {
            preview_pages: vec![],
            ..Default::default()
        },
    }
}

/// The real binary under test.
///
/// Not `SpawnConfig::new()`: inside an integration test `current_exe()` is the *test*
/// harness, so spawning it with `--compile-worker` would re-run libtest instead of
/// compiling anything. Cargo exports the path to the actual binary for this.
fn config() -> SpawnConfig {
    SpawnConfig::for_exe(env!("CARGO_BIN_EXE_typst-mcp"))
}

fn service(timeout: Duration) -> CompileService {
    let mut config = config();
    config.timeout = timeout;
    CompileService::new(config)
}

/// Compile something trivial and insist it worked — the "is the service still alive"
/// probe that follows every containment test.
async fn assert_still_serving(service: &CompileService) {
    let result = service
        .compile(&job("= Still here"))
        .await
        .expect("the service must survive a contained failure");
    assert!(
        matches!(result, JobResult::Ok { pages: 1, .. }),
        "expected a healthy compile afterwards, got {result:?}"
    );
}

/// A document that runs far longer than any deadline we would set.
///
/// Deliberately *not* `#while true {}`: Typst detects that itself and reports "loop
/// seems to be infinite", so it never reaches the deadline and would test nothing.
/// The real risk is the case its guard cannot catch — a finite computation that
/// simply takes too long. Measured on an M-series laptop: 40M iterations ≈ 4.3s,
/// so 400M is well past any deadline in these tests.
///
/// Equally deliberately **nested**, rather than one `range(300000000)`. Typst's
/// `range` materialises the whole array before the loop runs, so a single huge
/// range is a *memory* bomb, not a slow computation: under the worker's
/// `RLIMIT_AS` on Linux it dies allocating, in under a second, and the request
/// comes back 500 instead of 504. macOS does not enforce that limit the same way,
/// so the single-range version passed locally and failed only in CI. Nested ranges
/// of 20 000 keep the allocation at a few hundred KiB and put the cost in the
/// iteration, which is what the deadline is supposed to catch.
const RUNAWAY: &str =
    "#let acc = 0\n#for i in range(20000) { for j in range(20000) { acc = acc + 1 } }\n#acc";

#[tokio::test]
async fn typst_catches_trivial_infinite_loops_itself() {
    let _guard = exclusive().await;
    // Documenting upstream behaviour we rely on but do not control. If a future Typst
    // drops this guard, the deadline below is what still holds the line — but it is
    // worth knowing which layer caught it.
    let service = service(Duration::from_secs(30));
    let result = service
        .compile(&job("#while true {}"))
        .await
        .expect("worker ran");
    assert!(
        matches!(result, JobResult::Failed { .. }),
        "expected Typst's own loop guard to reject this: {result:?}"
    );
}

#[tokio::test]
async fn a_runaway_compile_is_killed_and_the_service_survives() {
    let _guard = exclusive().await;
    let service = service(Duration::from_secs(3));
    let started = Instant::now();

    let err = service
        .compile(&job(RUNAWAY))
        .await
        .expect_err("a document past the deadline must not return a result");

    assert!(
        matches!(err, SpawnError::Timeout { .. }),
        "expected a timeout, got {err:?}"
    );
    // Generous, but it must be bounded: the point is that the deadline is enforced by
    // killing a process, not by politely asking a thread to stop.
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "kill took {:?}; the deadline is not being enforced",
        started.elapsed()
    );

    assert_still_serving(&service).await;
}

#[tokio::test]
async fn deeply_recursive_documents_are_contained() {
    let _guard = exclusive().await;
    // Stack exhaustion rather than an infinite loop: a different way to die, and the
    // worker boundary has to contain it just the same.
    let service = service(Duration::from_secs(5));
    let source = "#let f(n) = f(n + 1)\n#f(0)";

    match service.compile(&job(source)).await {
        // Typst catches the recursion itself: a clean failure with diagnostics.
        Ok(JobResult::Failed { .. }) => {}
        // Or the worker dies on a stack overflow, which is exactly what it is for.
        Err(SpawnError::Died { .. } | SpawnError::Timeout { .. }) => {}
        other => panic!("recursion must not succeed or hang: {other:?}"),
    }

    assert_still_serving(&service).await;
}

#[tokio::test]
async fn the_filesystem_is_not_reachable() {
    let _guard = exclusive().await;
    let service = service(Duration::from_secs(10));

    // Every one of these resolves through `World::file`, which only ever consults the
    // in-memory bundle. There is no path from a document to the real filesystem.
    let attempts = [
        r#"#read("/etc/passwd")"#,
        r#"#read("../../../../etc/passwd")"#,
        r#"#read("/etc/hosts")"#,
        r#"#image("../../secrets.png")"#,
        r#"#include "/etc/passwd""#,
        r#"#import "../../../lib.typ": *"#,
    ];

    for source in attempts {
        let result = service.compile(&job(source)).await.expect("worker ran");
        let JobResult::Failed {
            message,
            diagnostics,
        } = &result
        else {
            panic!("{source} must not compile: {result:?}");
        };

        // The strongest available check: real /etc/passwd content starts "root:", and
        // real /etc/hosts contains "localhost". Neither may appear anywhere in what we
        // hand back, including inside a diagnostic that echoes the file.
        let haystack = format!("{message} {diagnostics:?}");
        for leaked in ["root:", "localhost", "/bin/bash"] {
            assert!(
                !haystack.contains(leaked),
                "{source} leaked {leaked:?} into the response: {haystack}"
            );
        }
    }

    assert_still_serving(&service).await;
}

#[tokio::test]
async fn package_imports_fail_with_an_explanation() {
    let _guard = exclusive().await;
    let service = service(Duration::from_secs(10));

    // No package downloader is linked in, so this cannot reach the network. What
    // matters for a model is that the error says so rather than looking transient —
    // otherwise it retries the same import forever.
    let result = service
        .compile(&job(r#"#import "@preview/cetz:0.3.1": *"#))
        .await
        .expect("worker ran");

    let JobResult::Failed {
        message,
        diagnostics,
    } = &result
    else {
        panic!("packages must not resolve: {result:?}");
    };
    let haystack = format!("{message} {diagnostics:?}");
    assert!(
        haystack.contains("not available"),
        "the error must explain that packages are unavailable: {haystack}"
    );
}

#[tokio::test]
async fn memory_bombs_hit_the_worker_limit_not_the_host() {
    let _guard = exclusive().await;
    // A modest cap so the test is quick and cannot disturb the machine running it.
    let mut bomb = job("#let big = range(200000000).map(x => str(x))\n#big.len()");
    bomb.limits.memory_bytes = 128 * 1024 * 1024;

    let service = service(Duration::from_secs(20));
    match service.compile(&bomb).await {
        // Allocation failure aborts the worker, which is the intended outcome.
        Err(SpawnError::Died { .. } | SpawnError::Timeout { .. }) => {}
        // Or Typst refuses it first, which is just as good.
        Ok(JobResult::Failed { .. }) => {}
        other => panic!("a memory bomb must not succeed: {other:?}"),
    }

    assert_still_serving(&service).await;
}

#[tokio::test]
async fn oversized_documents_are_refused_rather_than_rendered() {
    let _guard = exclusive().await;
    let mut long = job("#for _ in range(500) { pagebreak() }");
    long.limits.max_pages = 10;

    let service = service(Duration::from_secs(20));
    let result = service.compile(&long).await.expect("worker ran");

    let JobResult::Failed { message, .. } = &result else {
        panic!("a document over the page cap must not be returned: {result:?}");
    };
    assert!(message.contains("limit is 10"), "{message}");
}

#[tokio::test]
async fn preview_dimensions_are_clamped() {
    let _guard = exclusive().await;
    // A five-metre page at the default scale would be a pixmap of billions of pixels.
    // Without a clamp the allocator, not the caller, decides what happens next.
    let mut huge = job("#set page(width: 5000mm, height: 5000mm)\n= Big");
    huge.limits.preview_pages = vec![1];
    huge.limits.preview_max_px = 1000;

    let service = service(Duration::from_secs(20));
    let result = service.compile(&huge).await.expect("worker ran");

    let JobResult::Ok { previews, .. } = &result else {
        panic!("expected a compile: {result:?}");
    };
    let preview = previews.first().expect("one preview");
    assert!(
        preview.width <= 1000 && preview.height <= 1000,
        "preview was {}x{}, over the 1000px cap",
        preview.width,
        preview.height
    );
}

#[tokio::test]
async fn concurrent_compiles_are_bounded_and_all_complete() {
    let _guard = exclusive().await;
    let mut config = config();
    config.max_concurrent = 2;
    config.timeout = Duration::from_secs(30);
    config.queue_timeout = Duration::from_secs(30);
    let service = CompileService::new(config);

    // More work than permits: every job must still finish, just not all at once.
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let service = service.clone();
            tokio::spawn(async move { service.compile(&job(&format!("= Document {i}"))).await })
        })
        .collect();

    for handle in handles {
        let result = handle.await.expect("task").expect("compile");
        assert!(
            matches!(result, JobResult::Ok { pages: 1, .. }),
            "{result:?}"
        );
    }
}

#[tokio::test]
async fn a_broken_document_is_a_result_not_a_worker_failure() {
    let _guard = exclusive().await;
    // The distinction the protocol exists to preserve: "your document is wrong" comes
    // back as data with diagnostics, "the worker broke" comes back as an error. A
    // caller that cannot tell them apart cannot report either one usefully.
    let service = service(Duration::from_secs(10));
    let result = service
        .compile(&job("#let x ="))
        .await
        .expect("worker itself must be fine");

    let JobResult::Failed { diagnostics, .. } = &result else {
        panic!("expected a failed document: {result:?}");
    };
    let first = diagnostics.first().expect("a diagnostic");
    assert_eq!(first.file.as_deref(), Some("main.typ"));
    assert!(
        first.line.is_some(),
        "diagnostics must carry a position: {first:?}"
    );
}
