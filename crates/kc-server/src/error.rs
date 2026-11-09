//! Mapping between [`kc_core::Error`] and the JSON error shape of `docs/API.md`.
//!
//! Every HTTP failure goes through [`ApiError`]: `{ "error": { "code": ...,
//! "message": ... } }`. The `code` is one of the documented, stable strings; the
//! message is actionable and must never contain an API key or auth token.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An HTTP error with the documented JSON body.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "permission_denied", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "authentication required: sign in, or send `Authorization: Bearer <token>`",
        )
    }

    /// A 401 that says *why* without revealing whether a credential exists.
    pub fn unauthorized_message(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    /// `429 too_many_requests`, used for a sign-in lockout.
    pub fn too_many_requests(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "too_many_requests", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        // `docs/API.md` has no separate `conflict` code; a concurrent run is a
        // rejected request, so it keeps the documented `bad_request` code.
        Self::new(StatusCode::CONFLICT, "bad_request", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// `413 payload_too_large`. axum's own body-limit rejection is plain text,
    /// which would break the documented error contract, so the routes layer
    /// rewrites it through this constructor (see `routes::json_error_shape`).
    pub fn payload_too_large(max: usize, detail: Option<&str>) -> Self {
        let message = match detail {
            Some(detail) if !detail.is_empty() => {
                format!("the request body is too large; the maximum is {max} bytes ({detail})")
            }
            _ => format!("the request body is too large; the maximum is {max} bytes"),
        };
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": { "code": self.code, "message": self.message }
        });
        (self.status, Json(body)).into_response()
    }
}

impl From<kc_core::Error> for ApiError {
    fn from(error: kc_core::Error) -> Self {
        use kc_core::Error;
        match error {
            Error::Unauthorized => ApiError::unauthorized(),
            Error::BadRequest(message) => ApiError::bad_request(message),
            Error::NotFound(message) => ApiError::not_found(message),
            Error::PermissionDenied(message) => ApiError::forbidden(message),
            Error::PathEscape { .. } => {
                ApiError::new(StatusCode::BAD_REQUEST, "path_escape", error.to_string())
            }
            Error::Config(message) => ApiError::new(StatusCode::BAD_REQUEST, "config", message),
            Error::Provider(message) => ApiError::new(
                StatusCode::BAD_GATEWAY,
                "provider",
                format!("provider: {message}"),
            ),
            Error::Tool(message) => {
                ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "tool", message)
            }
            Error::Plugin(message) => ApiError::new(
                StatusCode::BAD_GATEWAY,
                "plugin",
                format!("plugin: {message}"),
            ),
            Error::Store(message) => ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store",
                format!("storage: {message}"),
            ),
            Error::Timeout(message) => ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "timeout",
                format!("timed out: {message}"),
            ),
            // `cancelled` and `io` are deliberately not exposed as codes: the
            // documented set has no entry for them.
            Error::Cancelled => ApiError::new(
                StatusCode::CONFLICT,
                "bad_request",
                "the operation was cancelled",
            ),
            Error::Io { path, source } => {
                ApiError::internal(format!("io error at {}: {source}", path.display()))
            }
            Error::Internal(message) => ApiError::internal(message),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Remove a literal secret from a message before it can reach the browser. The
/// provider and store layers already redact, this is the last line of defence.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        let secret = secret.trim();
        if secret.len() >= 4 {
            out = out.replace(secret, "***");
        }
    }
    // `Authorization: Bearer ...` and bare bearer tokens in upstream errors.
    let mut cleaned = String::with_capacity(out.len());
    for line in out.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("bearer ") {
            let keep = &line[..pos + "bearer ".len()];
            cleaned.push_str(keep);
            cleaned.push_str("***");
        } else {
            cleaned.push_str(line);
        }
        cleaned.push('\n');
    }
    cleaned.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kc_core::Error;

    #[test]
    fn error_codes_match_the_documented_set() {
        let documented = [
            "bad_request",
            "not_found",
            "permission_denied",
            "unauthorized",
            "config",
            "provider",
            "tool",
            "store",
            "plugin",
            "timeout",
            "path_escape",
            "payload_too_large",
            "internal",
        ];
        let cases: Vec<Error> = vec![
            Error::Config("x".into()),
            Error::BadRequest("x".into()),
            Error::NotFound("x".into()),
            Error::PermissionDenied("x".into()),
            Error::Unauthorized,
            Error::Cancelled,
            Error::Timeout("x".into()),
            Error::PathEscape {
                path: "/etc/passwd".into(),
                root: "/w".into(),
            },
            Error::Provider("x".into()),
            Error::Tool("x".into()),
            Error::Store("x".into()),
            Error::Plugin("x".into()),
            Error::Io {
                path: "/tmp/x".into(),
                source: std::io::Error::other("boom"),
            },
            Error::Internal("x".into()),
        ];
        for error in cases {
            let api: ApiError = error.into();
            assert!(
                documented.contains(&api.code),
                "undocumented code {}",
                api.code
            );
        }
    }

    #[test]
    fn status_codes_are_sensible() {
        let to = |e: Error| -> StatusCode { ApiError::from(e).status };
        assert_eq!(to(Error::BadRequest("x".into())), StatusCode::BAD_REQUEST);
        assert_eq!(to(Error::NotFound("x".into())), StatusCode::NOT_FOUND);
        assert_eq!(
            to(Error::PermissionDenied("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(to(Error::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(
            to(Error::PathEscape {
                path: "/etc".into(),
                root: "/w".into()
            }),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            to(Error::Store("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(to(Error::Provider("x".into())), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn payload_too_large_uses_the_documented_code_and_status() {
        let error = ApiError::payload_too_large(1024, None);
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(error.code, "payload_too_large");
        assert!(error.message.contains("1024"));
        let with_detail = ApiError::payload_too_large(1024, Some("length limit exceeded"));
        assert!(with_detail.message.contains("length limit exceeded"));
    }

    #[test]
    fn redaction_removes_keys_and_bearer_values() {
        let text = "upstream failed: Authorization: Bearer sk-secret-123 for key sk-secret-123";
        let out = redact(text, &["sk-secret-123"]);
        assert!(!out.contains("sk-secret-123"));
        assert!(out.contains("***"));
    }
}
