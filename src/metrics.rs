//! Bounded, dependency-light Prometheus metrics.
//!
//! Labels are deliberately fixed or reduced before they reach this module. Source text,
//! data values, filenames, diagnostics and credentials have no place in this API.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State as AxumState;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;

use crate::render::RenderError;
use crate::spawn::SpawnError;
use crate::store::Store;

const DURATION_BUCKETS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 60.0];
const PAGE_BUCKETS: &[f64] = &[1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 200.0];
const BYTE_BUCKETS: &[f64] = &[
    16_384.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
    4_194_304.0,
    16_777_216.0,
];

#[derive(Debug, Default)]
pub struct Metrics {
    state: Mutex<State>,
    compiles_in_flight: AtomicU64,
}

#[derive(Debug, Default)]
struct State {
    compiles: BTreeMap<(String, &'static str), u64>,
    compile_duration: Histogram,
    compile_pages: Histogram,
    pdf_bytes: Histogram,
    compile_kills: BTreeMap<&'static str, u64>,
    uploads: BTreeMap<&'static str, u64>,
    auth_failures: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Default)]
struct Histogram {
    bucket_counts: Vec<u64>,
    count: u64,
    sum: f64,
}

impl Histogram {
    fn observe(&mut self, value: f64, buckets: &[f64]) {
        if self.bucket_counts.len() < buckets.len() {
            self.bucket_counts.resize(buckets.len(), 0);
        }
        for (index, upper) in buckets.iter().enumerate() {
            if value <= *upper {
                self.bucket_counts[index] += 1;
            }
        }
        self.count += 1;
        self.sum += value;
    }
}

impl Metrics {
    pub fn start_compile(self: &Arc<Self>, template: &str) -> CompileObservation {
        self.compiles_in_flight.fetch_add(1, Ordering::Relaxed);
        CompileObservation {
            metrics: Arc::clone(self),
            template: bounded_template_label(template),
            started: Instant::now(),
            finished: false,
        }
    }

    pub fn upload(&self, kind: &'static str) {
        *self.lock().uploads.entry(kind).or_default() += 1;
    }

    pub fn auth_failure(&self, reason: &'static str) {
        *self.lock().auth_failures.entry(reason).or_default() += 1;
    }

    pub fn render(&self, store: &Store) -> String {
        let state = self.lock();
        let mut out = String::with_capacity(4096);
        out.push_str("# HELP typst_mcp_compiles_total Compile attempts by outcome and bounded template label.\n");
        out.push_str("# TYPE typst_mcp_compiles_total counter\n");
        for ((template, outcome), value) in &state.compiles {
            let _ = writeln!(
                out,
                "typst_mcp_compiles_total{{outcome=\"{outcome}\",template=\"{}\"}} {value}",
                escape_label(template)
            );
        }
        write_histogram(
            &mut out,
            "typst_mcp_compile_duration_seconds",
            "Compile wall time in seconds.",
            &state.compile_duration,
            DURATION_BUCKETS,
        );
        write_histogram(
            &mut out,
            "typst_mcp_compile_pages",
            "Pages produced by successful compiles.",
            &state.compile_pages,
            PAGE_BUCKETS,
        );
        write_histogram(
            &mut out,
            "typst_mcp_pdf_bytes",
            "PDF bytes produced by successful compiles.",
            &state.pdf_bytes,
            BYTE_BUCKETS,
        );
        out.push_str("# HELP typst_mcp_compile_kills_total Compile workers killed or lost.\n");
        out.push_str("# TYPE typst_mcp_compile_kills_total counter\n");
        for (reason, value) in &state.compile_kills {
            let _ = writeln!(
                out,
                "typst_mcp_compile_kills_total{{reason=\"{reason}\"}} {value}"
            );
        }
        out.push_str("# HELP typst_mcp_compiles_in_flight Compile slots currently occupied.\n");
        out.push_str("# TYPE typst_mcp_compiles_in_flight gauge\n");
        let _ = writeln!(
            out,
            "typst_mcp_compiles_in_flight {}",
            self.compiles_in_flight.load(Ordering::Relaxed)
        );
        out.push_str("# HELP typst_mcp_store_bytes Bytes held per opaque tenant partition.\n");
        out.push_str("# TYPE typst_mcp_store_bytes gauge\n");
        for (tenant, bytes) in store.tenant_usage() {
            let _ = writeln!(
                out,
                "typst_mcp_store_bytes{{tenant_fp=\"{}\"}} {bytes}",
                tenant.as_str()
            );
        }
        out.push_str("# HELP typst_mcp_store_bytes_total Total bytes held in the store.\n");
        out.push_str("# TYPE typst_mcp_store_bytes_total gauge\n");
        let _ = writeln!(out, "typst_mcp_store_bytes_total {}", store.used_bytes());
        out.push_str("# HELP typst_mcp_store_evictions_total Entries evicted by reason.\n");
        out.push_str("# TYPE typst_mcp_store_evictions_total counter\n");
        let (expired, quota) = store.evictions();
        let _ = writeln!(
            out,
            "typst_mcp_store_evictions_total{{reason=\"expired\"}} {expired}"
        );
        let _ = writeln!(
            out,
            "typst_mcp_store_evictions_total{{reason=\"quota\"}} {quota}"
        );
        out.push_str("# HELP typst_mcp_uploads_total Successful tenant uploads.\n");
        out.push_str("# TYPE typst_mcp_uploads_total counter\n");
        for (kind, value) in &state.uploads {
            let _ = writeln!(out, "typst_mcp_uploads_total{{kind=\"{kind}\"}} {value}");
        }
        out.push_str(
            "# HELP typst_mcp_auth_failures_total Authentication failures by public reason.\n",
        );
        out.push_str("# TYPE typst_mcp_auth_failures_total counter\n");
        for (reason, value) in &state.auth_failures {
            let _ = writeln!(
                out,
                "typst_mcp_auth_failures_total{{reason=\"{reason}\"}} {value}"
            );
        }
        out
    }

    fn finish_compile(
        &self,
        template: String,
        elapsed: f64,
        result: &Result<(usize, usize), &RenderError>,
    ) {
        self.compiles_in_flight.fetch_sub(1, Ordering::Relaxed);
        let mut state = self.lock();
        state.compile_duration.observe(elapsed, DURATION_BUCKETS);
        let outcome = match result {
            Ok((pages, bytes)) => {
                state.compile_pages.observe(*pages as f64, PAGE_BUCKETS);
                state.pdf_bytes.observe(*bytes as f64, BYTE_BUCKETS);
                "success"
            }
            Err(RenderError::Compile { .. }) => "compile_error",
            Err(RenderError::Spawn(SpawnError::Timeout { .. })) => {
                *state.compile_kills.entry("timeout").or_default() += 1;
                "timeout"
            }
            Err(RenderError::Spawn(SpawnError::Overloaded)) => "overloaded",
            Err(RenderError::Spawn(SpawnError::Died { .. })) => {
                *state.compile_kills.entry("worker_exit").or_default() += 1;
                "worker_error"
            }
            Err(
                RenderError::Spawn(SpawnError::Io(_))
                | RenderError::Workspace(_)
                | RenderError::Protocol(_),
            ) => "worker_error",
            Err(_) => "invalid",
        };
        *state.compiles.entry((template, outcome)).or_default() += 1;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

#[derive(Clone)]
struct EndpointState {
    metrics: Arc<Metrics>,
    store: Arc<Store>,
}

/// The isolated metrics surface: one route and no application or authentication state.
pub fn router(metrics: Arc<Metrics>, store: Arc<Store>) -> axum::Router {
    axum::Router::new()
        .route("/metrics", get(metrics_endpoint))
        .with_state(EndpointState { metrics, store })
}

async fn metrics_endpoint(AxumState(state): AxumState<EndpointState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        state.metrics.render(&state.store),
    )
}

pub struct CompileObservation {
    metrics: Arc<Metrics>,
    template: String,
    started: Instant,
    finished: bool,
}

impl CompileObservation {
    pub fn finish(mut self, result: &Result<(usize, usize), &RenderError>) {
        self.metrics.finish_compile(
            std::mem::take(&mut self.template),
            self.started.elapsed().as_secs_f64(),
            result,
        );
        self.finished = true;
    }
}

impl Drop for CompileObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.metrics
                .compiles_in_flight
                .fetch_sub(1, Ordering::Relaxed);
            *self
                .metrics
                .lock()
                .compiles
                .entry((std::mem::take(&mut self.template), "cancelled"))
                .or_default() += 1;
        }
    }
}

fn bounded_template_label(value: &str) -> String {
    match value {
        "source" | "template_validation" | "hanso" | "ksc" | "lenno" | "freudenberg" => {
            value.to_owned()
        }
        value if value.starts_with("tpl_") => "ephemeral".to_owned(),
        _ => "other".to_owned(),
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn write_histogram(
    out: &mut String,
    name: &str,
    help: &str,
    histogram: &Histogram,
    buckets: &[f64],
) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} histogram");
    for (index, upper) in buckets.iter().enumerate() {
        let count = histogram.bucket_counts.get(index).copied().unwrap_or(0);
        let _ = writeln!(out, "{name}_bucket{{le=\"{upper}\"}} {count}");
    }
    let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {}", histogram.count);
    let _ = writeln!(out, "{name}_sum {}", histogram.sum);
    let _ = writeln!(out, "{name}_count {}", histogram.count);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Limits;
    use tower::ServiceExt;

    #[test]
    fn labels_are_bounded_before_they_reach_prometheus() {
        assert_eq!(bounded_template_label("hanso"), "hanso");
        assert_eq!(
            bounded_template_label("tpl_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "ephemeral"
        );
        assert_eq!(bounded_template_label("customer_secret"), "other");
        assert_eq!(bounded_template_label(&"x".repeat(1000)), "other");
        assert_eq!(bounded_template_label("bad\nlabel"), "other");
    }

    #[test]
    fn exposition_contains_only_bounded_operational_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), Limits::default()).expect("store");
        let metrics = Arc::new(Metrics::default());
        metrics.auth_failure("rejected");
        metrics.upload("asset");
        let output = metrics.render(&store);
        assert!(output.contains("typst_mcp_auth_failures_total{reason=\"rejected\"} 1"));
        assert!(output.contains("typst_mcp_uploads_total{kind=\"asset\"} 1"));
        for forbidden in ["secret-token", "#set text", "customer data"] {
            assert!(!output.contains(forbidden));
        }
    }

    #[test]
    fn compile_outcomes_and_kills_are_recorded_without_document_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path(), Limits::default()).expect("store");
        let metrics = Arc::new(Metrics::default());
        metrics.start_compile("hanso").finish(&Ok((3, 42_000)));
        let timeout = RenderError::Spawn(SpawnError::Timeout {
            after: std::time::Duration::from_secs(20),
        });
        metrics
            .start_compile("tpl_01ARZ3NDEKTSV4RRFFQ69G5FAV")
            .finish(&Err(&timeout));
        let output = metrics.render(&store);
        assert!(output.contains("outcome=\"success\",template=\"hanso\"} 1"));
        assert!(output.contains("outcome=\"timeout\",template=\"ephemeral\"} 1"));
        assert!(output.contains("compile_kills_total{reason=\"timeout\"} 1"));
        assert!(output.contains("typst_mcp_compile_pages_count 1"));
    }

    #[test]
    fn histograms_retain_only_fixed_bucket_counts() {
        let mut histogram = Histogram::default();
        for value in 0..100_000 {
            histogram.observe(value as f64, PAGE_BUCKETS);
        }
        assert_eq!(histogram.bucket_counts.len(), PAGE_BUCKETS.len());
        assert_eq!(histogram.count, 100_000);
        assert_eq!(histogram.bucket_counts[0], 2);
        assert_eq!(histogram.bucket_counts[PAGE_BUCKETS.len() - 1], 201);
    }

    #[tokio::test]
    async fn metrics_router_exposes_only_the_prometheus_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(Store::open(dir.path(), Limits::default()).expect("store"));
        let app = router(Arc::new(Metrics::default()), store);
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/metrics")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("metrics response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let missing = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("missing response");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
