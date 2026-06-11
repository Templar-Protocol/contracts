use axum::response::{IntoResponse, Response};
use prometheus::{Encoder, TextEncoder};

pub async fn metrics() -> Response {
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();

    match encoder.encode(&metric_families, &mut buffer) {
        Ok(()) => (
            [(axum::http::header::CONTENT_TYPE, encoder.format_type())],
            buffer,
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to encode metrics: {error}"),
        )
            .into_response(),
    }
}
