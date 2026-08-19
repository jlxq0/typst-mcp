//! Structured audit envelopes with no content-shaped fields.
//!
//! An audit event can say who invoked which operation, whether it worked and how much
//! output it produced. It cannot accept source, data, rendered bytes, diagnostics or a
//! credential, so those values cannot be logged accidentally at a call site.

#[derive(Debug, Clone)]
pub struct AuditEvent<'a> {
    pub tenant_fp: &'a str,
    pub operation: &'static str,
    pub job_id: Option<&'a str>,
    pub template: Option<&'a str>,
    pub bytes: usize,
    pub pages: usize,
    pub duration_ms: u128,
    pub outcome: &'static str,
    pub diagnostic_count: usize,
}

impl AuditEvent<'_> {
    pub fn emit(&self) {
        tracing::info!(
            target: "typst_mcp::audit",
            audit = true,
            tenant_fp = safe_tenant(self.tenant_fp),
            operation = self.operation,
            job_id = safe_job(self.job_id),
            template = safe_template(self.template),
            bytes = self.bytes,
            pages = self.pages,
            duration_ms = self.duration_ms,
            outcome = self.outcome,
            diagnostic_count = self.diagnostic_count,
            "audit"
        );
    }
}

fn safe_tenant(value: &str) -> &str {
    if value.len() == 16 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        value
    } else {
        "invalid"
    }
}

fn safe_job(value: Option<&str>) -> &str {
    match value {
        Some(value)
            if value.starts_with("job_")
                && value.len() <= 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) =>
        {
            value
        }
        Some(_) => "invalid",
        None => "",
    }
}

fn safe_template(value: Option<&str>) -> &str {
    match value {
        Some(value @ ("hanso" | "ksc" | "lenno" | "freudenberg")) => value,
        Some(value) if value.starts_with("tpl_") => "ephemeral",
        Some(_) => "other",
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    struct Writer(Arc<Mutex<Vec<u8>>>);

    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Writer;

        fn make_writer(&'a self) -> Self::Writer {
            Writer(Arc::clone(&self.0))
        }
    }

    #[test]
    fn audit_logs_are_envelope_only() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(captured.clone())
            .finish();
        let source = "#set text(fill: red)[customer secret]";
        let data_value = "private account 1234";
        let credential = "sk_live_never_log_this";
        tracing::subscriber::with_default(subscriber, || {
            AuditEvent {
                tenant_fp: "a1b2c3d4e5f60708",
                operation: "typst_render",
                job_id: Some("job_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
                template: Some("hanso"),
                bytes: 42_000,
                pages: 3,
                duration_ms: 125,
                outcome: "success",
                diagnostic_count: 0,
            }
            .emit();
            // These values deliberately exist in the calling scope. The event API has
            // nowhere to put them.
            std::hint::black_box((source, data_value, credential));
        });
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).expect("utf8 log");
        assert!(log.contains("\"operation\":\"typst_render\""), "{log}");
        assert!(log.contains("\"diagnostic_count\":0"), "{log}");
        for forbidden in [source, data_value, credential] {
            assert!(
                !log.contains(forbidden),
                "sensitive value reached log: {log}"
            );
        }
    }

    #[test]
    fn request_derived_audit_labels_are_reduced_before_logging() {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(captured.clone())
            .finish();
        let source = "#set text(fill: red)[customer secret]";
        let data_value = "private account 1234";
        let credential = "sk_live_never_log_this";
        tracing::subscriber::with_default(subscriber, || {
            AuditEvent {
                tenant_fp: source,
                operation: "typst_render",
                job_id: Some(data_value),
                template: Some(credential),
                bytes: 0,
                pages: 0,
                duration_ms: 0,
                outcome: "error",
                diagnostic_count: 0,
            }
            .emit();
        });
        let log = String::from_utf8(captured.0.lock().unwrap().clone()).expect("utf8 log");
        assert!(log.contains("\"tenant_fp\":\"invalid\""), "{log}");
        assert!(log.contains("\"job_id\":\"invalid\""), "{log}");
        assert!(log.contains("\"template\":\"other\""), "{log}");
        for forbidden in [source, data_value, credential] {
            assert!(
                !log.contains(forbidden),
                "request-derived value reached log: {log}"
            );
        }
    }
}
