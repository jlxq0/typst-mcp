//! Wiring the pieces together and serving.

use std::sync::Arc;

use crate::api::{AppState, router};
use crate::auth::{ApiKeyAuth, OidcAuth};
use crate::config::Config;
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
        ));

        let state = AppState {
            api_key_auth: ApiKeyAuth::new(config.api_keys.clone()),
            oidc_auth: OidcAuth::new(config.oidc.clone(), &config.metadata_url()),
            render,
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

        tracing::info!(
            addr = %config.bind_addr,
            public_url = %config.public_url,
            templates = self.state.render.templates().len(),
            api_keys = config.api_keys.len(),
            oidc = self.state.oidc_auth.is_configured(),
            "typst-mcp listening"
        );

        self.spawn_reaper();

        let app = self
            .router()
            .layer(tower_http::trace::TraceLayer::new_for_http())
            .layer(axum::extract::DefaultBodyLimit::max(
                config.max_upload_bytes,
            ));

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
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

/// Set up logging from `TYPST_MCP_LOG_FORMAT` and `RUST_LOG`.
pub fn init_tracing() {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("typst_mcp=info,warn"));

    let json = std::env::var("TYPST_MCP_LOG_FORMAT")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(true);

    let registry = tracing_subscriber::registry().with(filter);
    if json {
        registry
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        registry.with(tracing_subscriber::fmt::layer()).init();
    }
}
