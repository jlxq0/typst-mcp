//! Optional OTLP trace export.
//!
//! The standard `OTEL_EXPORTER_OTLP_ENDPOINT` variable enables it. With no endpoint this
//! module constructs no exporter and opens no network connection.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};

pub struct Telemetry {
    provider: SdkTracerProvider,
    tracer: SdkTracer,
}

impl Telemetry {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "typst-mcp".to_owned());
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()?;
        let resource = Resource::builder()
            .with_service_name(service_name.clone())
            .with_attributes([
                KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                KeyValue::new("service.name", service_name.clone()),
            ])
            .build();
        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build();
        global::set_tracer_provider(provider.clone());
        let tracer = provider.tracer(service_name);
        Ok(Some(Self { provider, tracer }))
    }

    pub fn tracer(&self) -> SdkTracer {
        self.tracer.clone()
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Err(error) = self.provider.shutdown() {
            eprintln!("typst-mcp: OTLP shutdown failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_default_service_name_is_content_free() {
        // Keep the default a static operational identifier. In particular, never derive
        // it from a request, tenant or template name.
        assert_eq!("typst-mcp", env!("CARGO_PKG_NAME"));
    }
}
