//! Wiring the pieces together and serving.

use std::sync::Arc;

use crate::api::{AppState, router};
use crate::auth::{ApiKeyAuth, OidcAuth};
use crate::config::Config;
use crate::metrics::Metrics;
use crate::render::RenderService;
use crate::signing::Signer;
use crate::spawn::{CompileService, SpawnConfig};
use crate::store::Store;
use crate::templates::TemplateSet;

/// Everything constructed, ready to serve.
pub struct Server {
    pub state: AppState,
    store: Arc<Store>,
}

impl Server {
    /// Build from configuration.
    pub fn build(config: Config) -> anyhow::Result<Self> {
        let config = Arc::new(config);

        let store = Arc::new(Store::open(&config.data_dir, config.limits.clone())?);
        let metrics = Arc::new(Metrics::default());
        let templates = TemplateSet::load(&config.template_dir)?;

        let mut spawn = match &config.worker_exe {
            Some(exe) => SpawnConfig::for_exe(exe),
            None => SpawnConfig::new()?,
        };
        spawn.timeout = config.compile_timeout;
        spawn.max_concurrent = config.max_concurrent_compiles;

        let render = Arc::new(RenderService::new(
            Arc::clone(&config),
            CompileService::new(spawn),
            templates,
            Arc::clone(&store),
            Signer::new(config.signing_secret.clone()),
            Arc::clone(&metrics),
        ));

        let state = AppState {
            api_key_auth: ApiKeyAuth::with_metrics(config.api_keys.clone(), Arc::clone(&metrics)),
            oidc_auth: OidcAuth::with_metrics(
                config.oidc.clone(),
                &config.metadata_url(),
                Arc::clone(&metrics),
            ),
            render,
            metrics,
            config,
        };

        Ok(Self { state, store })
    }

    /// The axum router, for tests that drive it directly.
    pub fn router(&self) -> axum::Router {
        router(self.state.clone())
    }

    /// Serve until shutdown.
    pub async fn serve(self) -> anyhow::Result<()> {
        let config = Arc::clone(&self.state.config);
        let listener = tokio::net::TcpListener::bind(config.bind_addr).await?;
        let metrics_listener = tokio::net::TcpListener::bind(config.metrics_bind_addr).await?;

        tracing::info!(
            addr = %config.bind_addr,
            public_url = %config.public_url,
            templates = self.state.render.templates().len(),
            api_keys = config.api_keys.len(),
            oidc = self.state.oidc_auth.is_configured(),
            "typst-mcp listening"
        );
        tracing::info!(addr = %config.metrics_bind_addr, "metrics listening");

        self.spawn_reaper();

        // One line per request, at INFO.
        //
        // `TraceLayer::new_for_http()` on its own emits nothing here: its request and
        // response events are DEBUG, so with `RUST_LOG=typst_mcp=info` the server
        // logged its startup banner and then went silent. When an OAuth handshake
        // failed there was no record that `/authorize`, `/oauth/callback` or `/token`
        // had even been called, and the incident had to be reconstructed from Entra's
        // sign-in logs.
        //
        // **Path only, never the query string.** An authorization code arrives on the
        // query of `/oauth/callback`, and a code in a log file is a credential in a log
        // file — the default span formats the whole URI, which is precisely wrong here.
        let access_log = tower_http::trace::TraceLayer::new_for_http()
            .make_span_with(|request: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!(
                    "http",
                    method = %request.method(),
                    path = %request.uri().path(),
                )
            })
            .on_request(())
            .on_response(
                |response: &axum::http::Response<axum::body::Body>,
                 latency: std::time::Duration,
                 _: &tracing::Span| {
                    tracing::info!(
                        status = response.status().as_u16(),
                        latency_ms = latency.as_millis(),
                        "handled"
                    );
                },
            );

        let app = self
            .router()
            .layer(access_log)
            .layer(axum::extract::DefaultBodyLimit::max(
                config.max_upload_bytes,
            ));

        let metrics_app =
            crate::metrics::router(Arc::clone(&self.state.metrics), Arc::clone(&self.store));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            shutdown_signal().await;
            let _ = shutdown_tx.send(true);
        });
        let public = axum::serve(listener, app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_rx.clone()));
        let metrics = axum::serve(metrics_listener, metrics_app)
            .with_graceful_shutdown(wait_for_shutdown(shutdown_rx));
        tokio::try_join!(public, metrics)?;
        Ok(())
    }

    /// Delete expired entries on a timer.
    ///
    /// Retention is one of three independent guards on disk (the others being the
    /// per-tenant and global ceilings), so this running is not what keeps the volume
    /// bounded — it is what keeps documents from outliving their promised lifetime.
    fn spawn_reaper(&self) {
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let removed = store.reap();
                if removed > 0 {
                    tracing::info!(removed, used_bytes = store.used_bytes(), "reaped expired");
                }
            }
        });
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

async fn wait_for_shutdown(mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if !*shutdown.borrow() {
        let _ = shutdown.changed().await;
    }
}

/// Set up logging from `TYPST_MCP_LOG_FORMAT` and `RUST_LOG`.
pub fn init_tracing() -> anyhow::Result<Option<crate::telemetry::Telemetry>> {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("typst_mcp=info,warn"));

    let json = std::env::var("TYPST_MCP_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(true);

    let telemetry = crate::telemetry::Telemetry::from_env()?;
    if json {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json())
            .with(
                telemetry
                    .as_ref()
                    .map(|otel| tracing_opentelemetry::layer().with_tracer(otel.tracer())),
            )
            .try_init()?;
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .with(
                telemetry
                    .as_ref()
                    .map(|otel| tracing_opentelemetry::layer().with_tracer(otel.tracer())),
            )
            .try_init()?;
    }
    Ok(telemetry)
}
