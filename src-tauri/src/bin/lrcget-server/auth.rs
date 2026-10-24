use crate::Server;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use data_encoding::BASE64;

/// Requires HTTP basic auth on every request when credentials are configured.
pub async fn require_basic_auth(
    State(server): State<Server>,
    request: Request,
    next: Next,
) -> Response {
    let Some((username, password)) = &server.config.credentials else {
        return next.run(request).await;
    };

    let expected = BASE64.encode(format!("{username}:{password}").as_bytes());
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Basic "))
        .unwrap_or("");

    if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"LRCGET\", charset=\"UTF-8\"",
            )],
            "Authentication required",
        )
            .into_response()
    }
}

/// Compares without returning early, so response timing doesn't reveal how much of the
/// credentials matched.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}
