//! Metrics endpoint - Prometheus metrics exposition

use axum::{http::StatusCode, response::IntoResponse};

/// GET /metrics - Export Prometheus metrics
///
/// Returns metrics in Prometheus text format for scraping
#[tracing::instrument(name = "metrics")]
pub async fn metrics() -> impl IntoResponse {
    let metrics_text = crate::metrics::get_metrics();
    (
        StatusCode::OK,
        [("Content-Type", "text/plain; version=0.0.4")],
        metrics_text,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let response = metrics().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
