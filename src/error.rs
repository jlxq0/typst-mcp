//! One public error contract for HTTP and MCP.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rmcp::ErrorData as McpError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::Serialize;

use crate::auth::AuthError;
use crate::bundle::BundleError;
use crate::diagnostics::Diagnostic;
use crate::render::RenderError;
use crate::spawn::SpawnError;
use crate::store::StoreError;
use crate::templates::TemplateError;

/// Stable machine-readable failure categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    ProviderUnavailable,
    UnknownTemplate,
    NotFound,
    Expired,
    PayloadTooLarge,
    InvalidPath,
    InvalidBundle,
    InvalidData,
    CompileFailed,
    Timeout,
    Overloaded,
    QuotaExceeded,
    Internal,
}

/// The identical JSON object returned by HTTP and inside an MCP tool-error block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEnvelope {
    pub ok: bool,
    pub error: ErrorCode,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub available: Vec<String>,
}

/// A classified public error plus transport-only HTTP metadata.
#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    envelope: ErrorEnvelope,
    challenge: Option<String>,
}

impl ApiError {
    pub fn new(status: StatusCode, error: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            envelope: ErrorEnvelope {
                ok: false,
                error,
                message: message.into(),
                diagnostics: Vec::new(),
                available: Vec::new(),
            },
            challenge: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, message)
    }

    pub fn invalid_path(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::InvalidPath, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    pub fn unknown_template(name: &str, available: &[&str]) -> Self {
        let available = if available.is_empty() {
            "none".to_owned()
        } else {
            available.join(", ")
        };
        let mut error = Self::new(
            StatusCode::NOT_FOUND,
            ErrorCode::UnknownTemplate,
            format!("no template named {name:?}; available: {available}"),
        );
        error.envelope.available = available
            .split(", ")
            .filter(|name| *name != "none")
            .map(str::to_owned)
            .collect();
        error
    }

    pub fn expired(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GONE, ErrorCode::Expired, message)
    }

    pub fn auth(error: AuthError, challenge: &str) -> Self {
        let mut public = Self::new(
            error.status(),
            if error == AuthError::ProviderUnavailable {
                ErrorCode::ProviderUnavailable
            } else {
                ErrorCode::Unauthorized
            },
            error.message(),
        );
        if public.status == StatusCode::UNAUTHORIZED {
            public.challenge = Some(challenge.to_owned());
        }
        public
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn envelope(&self) -> &ErrorEnvelope {
        &self.envelope
    }

    pub fn into_mcp_result(self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::error(vec![ContentBlock::json(
            self.envelope,
        )?]))
    }

    fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        self.envelope.diagnostics = diagnostics;
        self
    }

    fn internal(kind: &'static str) -> Self {
        // Do not attach the source error: it may contain a host path, worker stderr,
        // source text, or another value that must not cross the public boundary.
        tracing::error!(kind, "internal request failure");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorCode::Internal,
            "the server could not complete this request",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.envelope)).into_response();
        if let Some(challenge) = self.challenge
            && let Ok(value) = HeaderValue::from_str(&challenge)
        {
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, value);
        }
        response
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Expired { .. } => Self::expired(error.to_string()),
            StoreError::NotFound { .. } | StoreError::BadId { .. } => {
                Self::not_found(error.to_string())
            }
            StoreError::TenantFull { .. } | StoreError::TenantEntriesFull { .. } => Self::new(
                StatusCode::INSUFFICIENT_STORAGE,
                ErrorCode::QuotaExceeded,
                error.to_string(),
            ),
            StoreError::Io { .. } => Self::internal("store_io"),
        }
    }
}

impl From<BundleError> for ApiError {
    fn from(error: BundleError) -> Self {
        match error {
            BundleError::TooManyFiles { .. } | BundleError::TooLarge { .. } => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
                error.to_string(),
            ),
            _ => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidBundle,
                error.to_string(),
            ),
        }
    }
}

impl From<TemplateError> for ApiError {
    fn from(error: TemplateError) -> Self {
        match error {
            TemplateError::Bundle(bundle) => bundle.into(),
            TemplateError::SchemaViolation(_)
            | TemplateError::BadArg { .. }
            | TemplateError::WrongKind { .. }
            | TemplateError::Value(_) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidData,
                error.to_string(),
            ),
            TemplateError::BadManifest { .. }
            | TemplateError::MissingEntrypoint { .. }
            | TemplateError::NoWrapperFn { .. }
            | TemplateError::BadWrapperFn { .. }
            | TemplateError::ReservedName(_)
            | TemplateError::UnknownOverride(_)
            | TemplateError::NonTextMetadata(_)
            | TemplateError::NameMismatch { .. }
            | TemplateError::NonFileMember { .. }
            | TemplateError::BadArchive(_)
            | TemplateError::BadPath { .. }
            | TemplateError::BadSchema { .. }
            | TemplateError::NoManifest(_) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidBundle,
                error.to_string(),
            ),
            TemplateError::Io { .. } => Self::internal("template_io"),
        }
    }
}

impl From<RenderError> for ApiError {
    fn from(error: RenderError) -> Self {
        match error {
            RenderError::UnknownTemplate { name, available } => {
                let names: Vec<&str> = available
                    .split(", ")
                    .filter(|name| !name.is_empty())
                    .collect();
                Self::unknown_template(&name, &names)
            }
            RenderError::Ambiguous | RenderError::Empty | RenderError::OverridesWithoutTemplate => {
                Self::bad_request(error.to_string())
            }
            RenderError::DuplicateAsset(_) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::InvalidBundle,
                error.to_string(),
            ),
            RenderError::Template(error) => error.into(),
            RenderError::Bundle(error) => error.into(),
            RenderError::Store(error) => error.into(),
            RenderError::Compile {
                message,
                diagnostics,
            } => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::CompileFailed,
                message,
            )
            .with_diagnostics(diagnostics),
            RenderError::Spawn(SpawnError::Timeout { after }) => Self::new(
                StatusCode::GATEWAY_TIMEOUT,
                ErrorCode::Timeout,
                format!(
                    "compile exceeded its {}s deadline and was killed; the document may contain an unbounded or excessively expensive computation",
                    after.as_secs()
                ),
            ),
            RenderError::Spawn(SpawnError::Overloaded) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Overloaded,
                "too many compiles are in flight; retry shortly",
            ),
            RenderError::Spawn(_) => Self::internal("compile_worker"),
            RenderError::Workspace(_) => Self::internal("compile_workspace"),
            RenderError::Protocol(_) => Self::internal("compile_protocol"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::bundle::MAX_FILES;
    use crate::diagnostics::Severity;
    use crate::store::Kind;

    #[test]
    fn public_classification_table_is_stable() {
        let cases = [
            (
                ApiError::from(BundleError::TooManyFiles {
                    count: MAX_FILES + 1,
                }),
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::PayloadTooLarge,
            ),
            (
                ApiError::from(StoreError::Expired {
                    kind: Kind::Output,
                    id: "job_old".into(),
                }),
                StatusCode::GONE,
                ErrorCode::Expired,
            ),
            (
                ApiError::from(RenderError::Spawn(SpawnError::Timeout {
                    after: Duration::from_secs(3),
                })),
                StatusCode::GATEWAY_TIMEOUT,
                ErrorCode::Timeout,
            ),
            (
                ApiError::from(RenderError::Spawn(SpawnError::Overloaded)),
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::Overloaded,
            ),
        ];

        for (error, status, code) in cases {
            assert_eq!(error.status(), status);
            assert_eq!(error.envelope().error, code);
            assert!(!error.envelope().ok);
        }
    }

    #[test]
    fn compile_diagnostics_survive_the_shared_mapping() {
        let diagnostic = Diagnostic::bare(Severity::Error, "expected expression");
        let error = ApiError::from(RenderError::Compile {
            message: "source did not compile".into(),
            diagnostics: vec![diagnostic.clone()],
        });
        assert_eq!(error.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.envelope().diagnostics, vec![diagnostic]);
    }

    #[test]
    fn internal_errors_expose_neither_paths_nor_worker_details() {
        let secret_path = "/private/customer/source.typ";
        let error = ApiError::from(RenderError::Workspace(io::Error::new(
            io::ErrorKind::PermissionDenied,
            secret_path,
        )));
        let json = serde_json::to_string(error.envelope()).expect("json");
        assert!(!json.contains(secret_path));
        assert!(!json.contains("PermissionDenied"));

        let error = ApiError::from(RenderError::Spawn(SpawnError::Died {
            exit: "signal 9".into(),
            stderr: "credential=super-secret source=customer text".into(),
        }));
        let json = serde_json::to_string(error.envelope()).expect("json");
        assert!(!json.contains("super-secret"));
        assert!(!json.contains("customer text"));

        let store = ApiError::from(StoreError::Io {
            path: PathBuf::from(secret_path),
            source: io::Error::other("database token"),
        });
        let json = serde_json::to_string(store.envelope()).expect("json");
        assert!(!json.contains(secret_path));
        assert!(!json.contains("database token"));
    }
}
